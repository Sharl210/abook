//! Audit: show what the tolerant parser had to absorb across the whole library,
//! and validate every record against the schema.

use crate::parse::Sev;
use crate::rec::{load_lib, Lib};
use crate::schema;
use crate::util::*;
use std::collections::BTreeMap;
use std::path::Path;

pub fn cmd_audit(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let libs: Vec<Lib> = match a.get("lib").unwrap_or("both") {
        "canonical" | "c" => vec![Lib::Canonical],
        "candidate" | "n" => vec![Lib::Candidate],
        _ => vec![Lib::Canonical, Lib::Candidate],
    };
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
        println!(
            "{{\"total\":{total},\"clean\":{clean},\"with_deviation\":{with_dev},\
\"with_error\":{with_err},\"rules\":{{{rules}}},\"fields\":{{{fields}}}}}"
        );
        return Ok(if with_err > 0 { 5 } else { 0 });
    }

    println!("审计 {total} 条记录");
    println!("  完全规范      {clean}");
    println!("  仅自动纠偏    {fixed_only}   （能被容错吸收、已留痕）");
    println!("  仅告警        {warn_only}   （能用，但召回质量下降）");
    println!("  含 ERROR      {with_err}   （必须修）");
    debug_assert_eq!(clean + fixed_only + warn_only + with_err, total);
    println!();
    if rule_counts.is_empty() {
        println!("没有任何规则被触发：真实库全部处于规范形态。");
    } else {
        println!("规则触发统计（ERROR / WARN / 自动纠偏）：");
        let mut v: Vec<_> = rule_counts.iter().collect();
        v.sort_by(|a, b| (b.1 .0 + b.1 .1 + b.1 .2).cmp(&(a.1 .0 + a.1 .1 + a.1 .2)));
        for (rule, (e, w, f)) in v {
            println!("  {rule:<24} {e:>4} / {w:>4} / {f:>4}");
        }
    }
    if !field_issue.is_empty() {
        println!();
        println!("按字段汇总：");
        let mut v: Vec<_> = field_issue.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        for (f, n) in v.iter().take(15) {
            println!("  {f:<20} {n}");
        }
    }
    if !samples.is_empty() && (a.has("verbose") || a.has("all")) {
        println!();
        println!("样例：");
        for s in &samples {
            println!("  {s}");
        }
    }
    Ok(if with_err > 0 { 5 } else { 0 })
}

/// Check a single file or stdin-like text for format problems without a library.
pub fn cmd_check(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let target = a.pos.first().cloned();
    let mut text = String::new();
    let mut label = String::new();
    let mut lib = Lib::Candidate;
    if let Some(t) = target {
        if t == "-" {
            use std::io::Read;
            std::io::stdin().read_to_string(&mut text)?;
            label = "<stdin>".into();
        } else {
            let (p, l) = crate::rec::resolve_record(&root, &t)?;
            text = read_str(&p)?;
            label = p.to_string_lossy().to_string();
            lib = l;
        }
    } else {
        use std::io::Read;
        std::io::stdin().read_to_string(&mut text)?;
        label = "<stdin>".into();
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
    println!("检查 {label}");
    println!("  {}", rep.summary());
    for d in &rep.devs {
        println!("  {}", d.show());
    }
    for i in &rep.issues {
        println!("  {}", i.show());
    }
    if rep.clean() {
        println!("  规范");
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
    let apply = a.has("apply");
    let dry = !apply;
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
        let done = schema::autofix(&mut p2, lib);
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

    for r in &report {
        println!("{r}");
    }
    println!(
        "{}：扫描 {scanned} 条，{} {changed} 条",
        if dry { "预演（加 --apply 才写入）" } else { "已修复" },
        if dry { "需修" } else { "改动" }
    );
    if !dry && changed > 0 {
        crate::cmd_admin::write_commit(
            &root,
            "repair-records",
            &format!("按校验器自动修复 {changed} 条记录的格式问题\n\n{}", report.join("\n")),
        )?;
    }
    Ok(0)
}
