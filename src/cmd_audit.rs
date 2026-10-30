//! Audit: show what the tolerant parser had to absorb across the whole library,
//! and validate every record against the schema.

use crate::parse::Sev;
use crate::rec::{load_lib, Lib};
use crate::schema;
use crate::util::*;
use std::collections::BTreeMap;

pub fn cmd_audit(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let libs: Vec<Lib> = crate::util::libs_from(a);
    let mut rule_counts: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new(); // (error,warn,fixed)
    let mut field_issue: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    let mut clean = 0usize;
    let mut with_dev = 0usize;
    let mut with_err = 0usize;
    // The four buckets below must partition `total`; a reader who adds them up
    // must reach the record count, otherwise the summary is misleading.
    let mut fixed_only = 0usize;
    let mut warn_only = 0usize;
    let mut samples: Vec<String> = Vec::new();

    for lib in &libs {
        for rec in load_lib(&root, *lib) {
            total += 1;
            let parsed = crate::parse::Parsed {
                lib_hint: Some(rec.lib),
                fields: rec.fields.clone(),
                body: rec.body.clone(),
                promotion: rec.promotion.clone(),
                attach: None,
                devs: rec.devs.clone(),
                had_header_tag: !rec.header_inferred,
                body_inferred: false,
            };
            let rep = schema::validate(&parsed, rec.lib, Some(&rec.file_name()));
            if !rec.devs.is_empty() {
                with_dev += 1;
            }
            if rep.has_error() {
                with_err += 1;
            } else if rep.clean() {
                clean += 1;
            } else if rep.warn_count() == 0 && rep.fixed_count() > 0 {
                fixed_only += 1;
            } else {
                warn_only += 1;
            }
            for d in &rec.devs {
                let e = rule_counts.entry(format!("{} dev", d.rule)).or_insert((0, 0, 0));
                match d.sev {
                    Sev::Error => e.0 += 1,
                    Sev::Warn => e.1 += 1,
                    Sev::Fixed => e.2 += 1,
                }
                if samples.len() < 40 && d.sev != Sev::Fixed {
                    samples.push(format!("{} :: {}", rec.file_name(), d.show()));
                }
            }
            for i in &rep.issues {
                let e = rule_counts.entry(i.rule.to_string()).or_insert((0, 0, 0));
                match i.sev {
                    Sev::Error => e.0 += 1,
                    Sev::Warn => e.1 += 1,
                    Sev::Fixed => e.2 += 1,
                }
                *field_issue.entry(i.field.clone()).or_insert(0) += 1;
                if samples.len() < 40 && i.sev == Sev::Error {
                    samples.push(format!("{} :: {}", rec.file_name(), i.show()));
                }
            }
        }
    }

    if a.has("json") {
        let rules = rule_counts
            .iter()
            .map(|(k, v)| {
                format!("{}:{{\"error\":{},\"warn\":{},\"fixed\":{}}}", jstr(k), v.0, v.1, v.2)
            })
            .collect::<Vec<_>>()
            .join(",");
        let fields = field_issue
            .iter()
            .map(|(k, v)| format!("{}:{}", jstr(k), v))
            .collect::<Vec<_>>()
            .join(",");
        crate::say_line!(
            "{{\"total\":{total},\"clean\":{clean},\"with_deviation\":{with_dev},\
\"with_error\":{with_err},\"rules\":{{{rules}}},\"fields\":{{{fields}}}}}"
        );
        return Ok(if with_err > 0 { 5 } else { 0 });
    }

    crate::say_line!("审计 {total} 条记录");
    crate::say_line!("  完全规范      {clean}");
    crate::say_line!("  仅内存归一    {fixed_only}   （能被容错吸收、已留痕）");
    crate::say_line!("  仅告警        {warn_only}   （能用，但召回质量下降）");
    crate::say_line!("  含 ERROR      {with_err}   （必须修）");
    // Enforced at runtime, not just in debug builds: a summary whose parts do
    // not add up to the total is actively misleading, and it shipped once.
    if clean + fixed_only + warn_only + with_err != total {
        // A summary whose parts do not add up cannot be handed to the caller at
        // all -- not even with a non-zero exit code and correct numbers, because
        // the numbers are the thing that is wrong. Route it through the error
        // channel so nothing downstream can accidentally print it.
        return crate::util::other(format!(
            "内部分桶不一致: {} != 记录总数 {total}",
            clean + fixed_only + warn_only + with_err
        ));
    }
    crate::say_line!();
    if rule_counts.is_empty() {
        crate::say_line!("没有任何规则被触发：真实库全部处于规范形态。");
    } else {
        crate::say_line!("规则触发统计（ERROR / WARN / 内存归一）：");
        let mut v: Vec<_> = rule_counts.iter().collect();
        v.sort_by(|a, b| (b.1 .0 + b.1 .1 + b.1 .2).cmp(&(a.1 .0 + a.1 .1 + a.1 .2)));
        for (rule, (e, w, f)) in v {
            // 偏差类的码自带 ` dev` 后缀（来自解析期 Deviation，不是 Issue），
            // 查说明时先剥掉，否则查不到。
            let base = rule.trim_end_matches(" dev");
            match crate::schema::rule_doc(base) {
                Some(doc) => crate::say_line!("  {rule:<10} {e:>4} / {w:>4} / {f:>4}  {doc}"),
                // 查不到就明说查不到。一个看起来已经解释过的码比没解释更糟：
                // 读者会以为自己懂了，于是不去看样例。
                None => crate::say_line!(
                    "  {rule:<10} {e:>4} / {w:>4} / {f:>4}  （此码未收录说明，用 --all 看样例）"
                ),
            }
        }
    }
    if !field_issue.is_empty() {
        crate::say_line!();
        crate::say_line!("按字段汇总：");
        let mut v: Vec<_> = field_issue.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        for (f, n) in v.iter().take(15) {
            crate::say_line!("  {f:<20} {n}");
        }
    }
    if !samples.is_empty() && (a.has("verbose") || a.has("all")) {
        crate::say_line!();
        crate::say_line!("样例：");
        for s in &samples {
            crate::say_line!("  {s}");
        }
    }
    Ok(if with_err > 0 { 5 } else { 0 })
}

/// Check a single file or stdin-like text for format problems without a library.
pub fn cmd_check(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let target = a.pos.first().cloned();
    let mut text = String::new();
    let label;
    let mut lib = Lib::Candidate;
    if let Some(t) = target {
        if t == "-" {
            use std::io::Read;
            std::io::stdin().read_to_string(&mut text)?;
            label = "<stdin>".to_string();
        } else {
            let (p, l) = crate::rec::resolve_record(&root, &t)?;
            text = read_str(&p)?;
            label = p.to_string_lossy().to_string();
            lib = l;
        }
    } else {
        // No positional argument. Reading stdin unconditionally was wrong: in a
        // terminal that returns immediately with an empty string, and the
        // validator then reports six bogus ERRORs ("无法解析为记录" etc.) with
        // exit code 5 -- indistinguishable from "your library is corrupt".
        // `check -` already means stdin explicitly, so only honour an implicit
        // stdin when something is actually piped in; otherwise say how to use it.
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            eprintln!("USAGE: check 需要一个记录，或 `check -` 从管道读；整库检查用 lint");
            eprintln!("  abook check <记录>      校验单条记录");
            eprintln!("  abook check -           校验管道送来的文本");
            eprintln!("  abook lint              整库风格检查");
            return Ok(2);
        }
        use std::io::Read;
        std::io::stdin().read_to_string(&mut text)?;
        label = "<stdin>".into();
        if text.trim().is_empty() {
            eprintln!("USAGE: 管道没有内容可校验（check - 需要 stdin 有文本）");
            return Ok(2);
        }
    }
    if let Some(l) = a.get("lib") {
        lib = if l.starts_with('c') && !l.starts_with("cand") {
            Lib::Canonical
        } else {
            Lib::Candidate
        };
    }
    let p = crate::parse::parse(&text, Some(lib));
    let rep = schema::validate(&p, p.lib_hint.unwrap_or(lib), None);
    if a.has("json") {
        let devs = rep
            .devs
            .iter()
            .map(|d| format!("{{\"detail\":{}}}", jstr(&d.show())))
            .collect::<Vec<_>>()
            .join(",");
        let issues = rep
            .issues
            .iter()
            .map(|i| format!("{{\"detail\":{}}}", jstr(&i.show())))
            .collect::<Vec<_>>()
            .join(",");
        crate::say_line!(
            "{{\"target\":{},\"summary\":{},\"clean\":{},\"has_error\":{},\"deviations\":[{devs}],\"issues\":[{issues}]}}",
            jstr(&crate::util::rel(&root, std::path::Path::new(&label))),
            jstr(&rep.summary()),
            rep.clean(),
            rep.has_error()
        );
        return Ok(if rep.clean() {
            0
        } else if rep.has_error() {
            5
        } else {
            0
        });
    }
    crate::say_line!("检查 {label}");
    crate::say_line!("  {}", rep.summary());
    for d in &rep.devs {
        crate::say_line!("  {}", d.show());
    }
    for i in &rep.issues {
        crate::say_line!("  {}", i.show());
    }
    if rep.clean() {
        crate::say_line!("  规范");
        Ok(0)
    } else if rep.has_error() {
        Ok(5)
    } else {
        Ok(0)
    }
}

/// Fix one record in place (or a whole library) using the validator.
pub fn cmd_repair(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    // Dry by default. `repair` writes into every record that fails a check,
    // and it used to do so the moment you ran it. Adding a `KEYS=`/`SIZE=` line
    // to 39 shipped records -- which is what one stray invocation did -- is not
    // something to do on the strength of a command name.
    let dry = a.dry_run();
    let mut changed = 0usize;
    let mut scanned = 0usize;
    let mut report: Vec<String> = Vec::new();

    let targets: Vec<(std::path::PathBuf, Lib)> = if let Some(sel) = a.pos.first() {
        vec![crate::rec::resolve_record(&root, sel)?]
    } else {
        let mut v = Vec::new();
        for lib in [Lib::Canonical, Lib::Candidate] {
            for r in load_lib(&root, lib) {
                v.push((r.path.clone(), lib));
            }
        }
        v
    };

    for (path, lib) in targets {
        scanned += 1;
        let raw = read_str(&path)?;
        let parsed = crate::parse::parse(&raw, Some(lib));
        if parsed.has_error() {
            report.push(format!(
                "跳过 {}（有无法自动修的 ERROR）",
                path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
            ));
            continue;
        }
        let mut p2 = parsed.clone();
        let fname = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let only: Vec<String> = match a.get("only") {
            Some(v) => v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
            None => Vec::new(),
        };
        let done = schema::autofix(&mut p2, lib, Some(&fname), &only);
        if done.is_empty() {
            continue;
        }
        changed += 1;
        let fname = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        report.push(format!("{fname}: {}", done.join(", ")));
        if !dry {
            let mut rec = crate::rec::Record::parse(&path, lib)?;
            for (k, v) in &p2.fields {
                rec.set(k, v);
            }
            write_atomic(&path, &rec.render())?;
            // read back
            let back = crate::rec::Record::parse(&path, lib)?;
            if back.get("FORMAT").is_empty() {
                return integrity(format!("修复后复读失败: {fname}"));
            }
        }
    }

    if a.has("json") {
        let items = report
            .iter()
            .map(|r| jstr(r))
            .collect::<Vec<_>>()
            .join(",");
        crate::say_line!(
            "{{\"scanned\":{scanned},\"changed\":{changed},\"applied\":{},\"details\":[{items}]}}",
            !dry
        );
        if dry {
            return Ok(if changed > 0 { 5 } else { 0 });
        }
        if changed > 0 {
            crate::cmd_admin::write_commit(
                &root,
                &crate::cmd_admin::CommitInfo {
                    slug: "repair-records",
                operation: "PATCH",
                scope: "批量修复记录格式，仅动不合规处",
                    library: "BOTH",
                    summary: &format!("按校验器自动修复 {changed} 条记录的格式问题"),
                    target: "canonical/records, candidate/records",
                    body: &format!(
                        "按校验器自动修复 {changed} 条记录的格式问题\n\n{}",
                        report.join("\n")
                    ),
                },
            )?;
        }
        return Ok(0);
    }
    for r in &report {
        crate::say_line!("{r}");
    }
    crate::say_line!(
        "{}：扫描 {scanned} 条，{} {changed} 条",
        if dry { "预演（加 --apply 才写入）" } else { "已修复" },
        if dry { "需修" } else { "改动" }
    );
    if !dry && changed > 0 {
        crate::cmd_admin::write_commit(
            &root,
            &crate::cmd_admin::CommitInfo {
                slug: "repair-records",
                operation: "PATCH",
                scope: "批量修复记录格式，仅动不合规处",
                library: "BOTH",
                summary: &format!("按校验器自动修复 {changed} 条记录的格式问题"),
                target: "canonical/records, candidate/records",
                body: &format!("按校验器自动修复 {changed} 条记录的格式问题\n\n{}", report.join("\n")),
            },
        )?;
    }
    Ok(0)
}

// ---------------------------------------------------------------- 引用完整性

/// `abook refs` — 全库检查引用是否指得着。
///
/// 为什么必须由工具做：`PROMOTED_FROM`、`RELATED_IDS`、`SUPERSEDES` 这些字段
/// 写的是**另一条记录的身份**。身份一旦对不上（记录被删、ID 被写坏、改名没
/// 同步），字段本身还是合法的非空字符串，逐条校验永远看不出问题——只有把全
/// 库的身份集建起来对一遍才能发现。
///
/// 这类悬空以前只能靠临时脚本核对，脚本还会因为不认旧库的
/// `ID=x;PROJECT=y` 打包写法而误报一大片。工具知道所有的历史形态，所以由它
/// 来判断，并且只报告不改写：**替换成什么是人的决定，不是工具的猜测**。
/// 值是否长得像一条记录的身份。
///
/// 身份形态：`<N|C|AB2-N|AB2-C>-<时间戳或序号>-<哈希或语义尾巴>`，无空格、
/// 无路径分隔符、无中文标点。旧库的语义尾巴（`quotepair`/`imeassist`）也算身份，
/// 因为它确实是记录名。
fn id_shaped(v: &str) -> bool {
    if v.contains('/') || v.contains(' ') || v.contains('.') {
        return false;
    }
    if v.chars().any(|c| c == '（' || c == '）' || c == '，' || c == '；' || c == '、') {
        return false;
    }
    let parts: Vec<&str> = v.split('-').collect();
    if parts.len() < 3 {
        return false;
    }
    let head_ok = matches!(parts[0], "N" | "C" | "AB2");
    let body_ok = parts[1..].iter().all(|p| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == ':' || c == 'T')
    });
    head_ok && body_ok
}

pub fn cmd_refs(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let libs: Vec<Lib> = crate::util::libs_from(a);

    // 身份集：文件名干 + ID 字段的全部别名。
    //
    // 必须两个来源都收：旧库靠 ID 字段认身（文件名是 `AB2-C-001` 这种顺序号），
    // 新库靠文件名认身。只认一个方向都会把另一半判成悬空。
    let mut known: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut all = Vec::new();
    for lib in &libs {
        for rec in load_lib(&root, *lib) {
            let stem = rec.file_name().trim_end_matches(".md").to_string();
            known.insert(stem.clone());
            // ID 字段可能是 `x` 也可能是 `x;PROJECT=y;TASK=z` 一整个打包行。
            let raw = rec.get("ID");
            for part in raw.split(';') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                match part.strip_prefix("ID=") {
                    Some(v) => {
                        known.insert(v.trim().to_string());
                    }
                    None if !part.contains('=') => {
                        known.insert(part.to_string());
                    }
                    None => {}
                }
            }
            all.push((rec, stem));
        }
    }

    // 会被当作"指向别处"的字段。它们分成两类，判据不同：
    //
    // **活关联**——值必须指得着，指不着就是数据问题：
    //   `PROMOTED_TO`（候选→正式，选中即指）、`SUPERSEDES`、`PREVIOUS_ID`（替代链）、
    //   `RELATED_IDS`（通用关联）。
    //
    // **历史来源**除外：`PROMOTED_FROM` 记录"我从哪来"，**允许指向已不存在的记录**。
    // 晋升改成移动语义后，候选在晋升成功时即被删除，所以每一条正式记录的
    // `PROMOTED_FROM` 都必然指着一个已消失的候选——这是正确状态，不是损坏。
    // 把它算进悬空，等于让每次晋升都自己制造一条告警，真悬空会被噪音淹没。
    // 溯源与活引用语义不同，不该共用一套检查。
    const REF_FIELDS: [&str; 4] =
        ["PROMOTED_TO", "SUPERSEDES", "PREVIOUS_ID", "RELATED_IDS"];

    let mut dangling: Vec<(String, String, String)> = Vec::new(); // (记录, 字段, 悬空值)
    let mut checked = 0usize;
    let mut other = 0usize; // 非身份值（文档路径/描述文本），不计入悬空
    for (rec, stem) in &all {
        for f in REF_FIELDS {
            let v = rec.get(f);
            if v.trim().is_empty() {
                continue;
            }
            for one in v.split([',', '，']) {
                let one = one.trim();
                if one.is_empty() {
                    continue;
                }
                checked += 1;
                if known.contains(one) {
                    continue;
                }
                // 不是所有非空值都是身份。
                //
                // 旧库的 `PROMOTED_FROM` 存的是**来源文档路径**
                // （`References/core/prompt-engineering.md#某节`），
                // `SUPERSEDES` 存的是**被取代内容的自然语言描述**
                // （`旧按Agent与复杂度默认激活的Aurorix行为`）。这两类值本来就
                // 不该拿去查记录，把它们报成"悬空"是分类错误，不是数据错误——
                // 初版就是这样多报了 117 处，让人以为库坏得很厉害。
                //
                // 判据按形状：身份是 `<前缀>-<时间戳>-<尾巴>` 的无空格三段结构；
                // 含路径分隔符、含空格、含中文标点的都是另一种东西。
                if !id_shaped(one) {
                    other += 1;
                    continue;
                }
                dangling.push((stem.clone(), f.to_string(), one.to_string()));
            }
        }
    }

    // ---- --fix：只在能唯一推导时改写，推不出来就留着
    //
    // 改名而不改引用是这类悬空的主因：旧身份带着语义尾巴
    // （`N-20260930T160500-quotepair`），改名后变成内容哈希
    // （`N-20260930T160500-3a969b6d`）。两者的**时间戳部分完全相同**——
    // 时间戳是建档那一刻，同前缀同时间戳就是同一条记录。
    //
    // 所以映射是可以推出来的，但只在唯一时才算数：同一秒建两条记录会让时间戳
    // 撞车，那时任何选择都是猜。**能推的不问、推不出的不猜**，是这个修复的
    // 全部原则。
    // `--json` 时所有人类可读的行都不能出现：调用方要把输出当 JSON 解析，
    // 混进一行 `C-xxx: PROMOTED_FROM → ...` 整个结果就废了。所以 --fix 的
    // 逐条报告在 json 模式下推迟到最后一起输出。
    let quiet = a.has("json");
    // `--fix` 本身即表示"要写"，只用 `--dry-run` 拦。
    //
    // `dry_run()` 的判定是「没有 apply/write/yes 就算预演」，它不认 `--fix`，
    // 于是 `refs --fix --approve` 被自己的预演分支当成"没说要写"而跳过——
    // 参数齐全却什么都不做，还打印 `[dry-run]`，等于告诉调用方"我在预演"。
    // 这类"参数名没被自己的开关函数认识"的坑，与 `--dry-run` 当初被忽略同源。
    let fix_dry = a.has("dry-run") || a.has("dryrun");
    if a.has("fix") && !dangling.is_empty() {
        use std::collections::BTreeMap;
        let mut by_stamp: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        for (_, stem) in &all {
            let parts: Vec<&str> = stem.split('-').collect();
            if parts.len() == 3 {
                by_stamp
                    .entry((parts[0].to_string(), parts[1].to_string()))
                    .or_default()
                    .push(stem.clone());
            }
        }
        let dry = fix_dry;

        // 「候选 ID → 它晋升成的正式 ID」。这是一条**精确映射**，不是推测：
        // 正式记录头部的 `PROMOTED_FROM` 就是"我从哪来"的事实记录。
        //
        // 为什么必须单独建这张表：移动语义下候选晋升后即被删除，于是
        // 「候选 A 引用候选 B、两者一起晋升」会让 A 的 `RELATED_IDS` 指着 B 的
        // 候选 ID——一条已删除的记录。而 A、B 的正式 ID 时间戳与候选 ID 的
        // 时间戳完全不同（晋升那一刻才生成），下面的时间戳推导接不上。
        // 只有 `PROMOTED_FROM` 能把它接起来。
        let mut promoted_map: BTreeMap<String, String> = BTreeMap::new();
        for (rec, stem) in &all {
            if rec.lib != Lib::Canonical {
                continue;
            }
            let from = rec.get("PROMOTED_FROM").trim().to_string();
            if !from.is_empty() {
                promoted_map.insert(from, stem.clone());
            }
        }

        let mut planned: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
        let mut unresolved: Vec<(String, String, String, &str)> = Vec::new();
        for (rec, field, val) in &dangling {
            // 先查"它是不是一条已晋升的候选"——这条映射是精确的，
            // 优先级高于下面的时间戳推导（后者是猜，前者是查表）。
            if let Some(canon) = promoted_map.get(val) {
                planned
                    .entry(rec.clone())
                    .or_default()
                    .push((field.clone(), val.clone(), canon.clone()));
                continue;
            }
            let parts: Vec<&str> = val.split('-').collect();
            if parts.len() != 3 {
                unresolved.push((rec.clone(), field.clone(), val.clone(), "形态无法解析"));
                continue;
            }
            let key = (parts[0].to_string(), parts[1].to_string());
            match by_stamp.get(&key).map(|v| v.as_slice()) {
                Some([one]) => {
                    // 大小写不同的哈希尾巴也归到这一条
                    planned
                        .entry(rec.clone())
                        .or_default()
                        .push((field.clone(), val.clone(), one.clone()));
                }
                Some(many) if many.len() > 1 => {
                    unresolved.push((rec.clone(), field.clone(), val.clone(), "同时间戳有多条，无法判定"));
                }
                _ => {
                    unresolved.push((rec.clone(), field.clone(), val.clone(), "目标记录不存在"));
                }
            }
        }

        let total_fix: usize = planned.values().map(|v| v.len()).sum();
        if !quiet {
            for (rec, subs) in &planned {
                for (field, old, new) in subs {
                    crate::say_line!("{rec}: {field} {old} → {new}");
                }
            }
            for (rec, field, val, why) in &unresolved {
                crate::say_line!("{rec}: {field} → {val}（{why}，保持原样）");
            }
            crate::say_line!(
                "\n可修 {} 处，无法判定 {} 处。",
                total_fix,
                unresolved.len()
            );
        }
        if dry {
            if quiet {
                crate::say_line!(
                    "{{\"planned\":{},\"unresolved\":{},\"applied\":0}}",
                    total_fix,
                    unresolved.len()
                );
            } else {
                crate::say_line!("[dry-run] 未写入。去掉 --dry-run 执行。");
            }
            return Ok(if total_fix > 0 { 5 } else { 0 });
        }
        if total_fix == 0 {
            return Ok(0);
        }
        if libs.contains(&Lib::Canonical) && !a.has("approve") {
            return blocked("修复正式库引用需要 --approve。");
        }
        let mut wrote = 0usize;
        for (rec, subs) in &planned {
            let path = match libs.iter().find_map(|l| {
                // `Lib::dir()` 返回的就是 `canonical/records`,别再拼一次
                let p = root.join(l.dir()).join(format!("{rec}.md"));
                if p.exists() {
                    Some((p, *l))
                } else {
                    None
                }
            }) {
                Some(x) => x,
                None => continue,
            };
            let mut r = crate::rec::Record::parse(&path.0, path.1)?;
            for (field, old, new) in subs {
                let cur = r.get(field);
                let rebuilt: Vec<String> = cur
                    .split([',', '，'])
                    .map(|x| x.trim())
                    .filter(|x| !x.is_empty())
                    .map(|x| if x == old { new.clone() } else { x.to_string() })
                    .collect();
                r.set(field, &rebuilt.join(","));
            }
            write_atomic(&path.0, &r.render())?;
            let back = crate::rec::Record::parse(&path.0, path.1)?;
            if back.get("TITLE").is_empty() {
                return integrity(format!("修复后复读失败: {rec}"));
            }
            wrote += 1;
        }
        if quiet {
            crate::say_line!(
                "{{\"planned\":{},\"unresolved\":{},\"applied\":{}}}",
                total_fix,
                unresolved.len(),
                wrote
            );
        } else {
            crate::say_line!("已修复 {wrote} 条记录的引用。");
        }
        return Ok(0);
    }

    if a.has("json") {
        let items = dangling
            .iter()
            .map(|(r, f, v)| {
                format!(
                    "{{\"record\":{},\"field\":{},\"target\":{}}}",
                    jstr(r),
                    jstr(f),
                    jstr(v)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        crate::say_line!(
            "{{\"records\":{},\"refs_checked\":{},\"dangling\":{},\"items\":[{items}]}}",
            all.len(),
            checked,
            dangling.len()
        );
    } else {
        if dangling.is_empty() {
            crate::say_line!(
                "引用完整性通过：{} 条记录、{} 个引用，全部指得着",
                all.len(),
                checked
            );
        } else {
            for (r, f, v) in &dangling {
                crate::say_line!("{r}: {f} → {v}");
            }
            crate::say_line!(
                "\n{} 条记录、{} 个引用中有 {} 个悬空。",
                all.len(),
                checked,
                dangling.len()
            );
            crate::say_line!(
                "悬空的常见成因：目标记录已删除、ID 被写坏（`abook repair --apply` 可对齐）、\
                 改名后引用未同步（`abook rename` 会自动改，手改文件名不会）。"
            );
            crate::say_line!(
                "工具只报告不改写：替换成哪一条是人的判断，猜错比留着更坏。\
                 确认目标后可用 `abook set <记录> <字段> <正确身份>` 修正。"
            );
        }
    }
    Ok(if dangling.is_empty() { 0 } else { 5 })
}
