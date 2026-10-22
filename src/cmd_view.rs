//! Viewing: paged listings, single-field read, history, diff, export, tree.

use crate::paging::{text_page, Page};
use crate::rec::{load_lib, resolve_record, ListFilter, Lib, Record};
use crate::util::*;
use std::path::Path;

/// `get <record> <field>` — read one field without dumping the whole record.
pub fn cmd_get(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let (path, lib) = resolve_record(&root, sel)?;
    let rec = Record::parse(&path, lib)?;
    match a.pos.get(1) {
        Some(f) => {
            let (canon, _) = crate::parse::canon_key(f);
            if canon == "BODY" || canon == "MEMORY_BODY" {
                println!("{}", rec.body);
                return Ok(0);
            }
            if !rec.has(&canon) {
                return notfound(format!("记录没有字段 {canon}"));
            }
            println!("{}", rec.get(&canon));
            Ok(0)
        }
        None => {
            // no field: list the field names only
            for (k, v) in &rec.fields {
                println!("{k}\t{}\t{}", v.chars().count(), truncate_chars(v, 60));
            }
            println!("BODY\t{}\t{}", rec.body.chars().count(), truncate_chars(&rec.body, 60));
            Ok(0)
        }
    }
}

/// `show` with paging and optional field filter.
pub fn cmd_show(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let (path, lib) = resolve_record(&root, sel)?;
    let rec = Record::parse(&path, lib)?;
    let p = if a.has("page") || a.has("per-page") {
        Some(Page::unchecked())
    } else {
        None
    };
    let _ = p;
    if a.has("json") {
        let fields = rec
            .fields
            .iter()
            .map(|(k, v)| format!("{}:{}", jstr(k), jstr(v)))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"path\":{},\"lib\":{},\"fields\":{{{fields}}},\"body\":{},\"deviations\":{}}}",
            jstr(&rel(&root, &path)),
            jstr(lib.name()),
            jstr(&rec.body),
            rec.devs.len()
        );
        return Ok(0);
    }
    println!("{}  [{}]", rel(&root, &path), lib.name());
    if a.has("fields") {
        for (k, v) in &rec.fields {
            println!("  {k}={v}");
        }
    } else {
        for (k, v) in &rec.fields {
            println!("  {k}={}", truncate_chars(v, 200));
        }
    }
    if !rec.body.is_empty() {
        println!("  [正文]");
        let (body, footer) = if a.has("page") || a.has("per-page") {
            text_page(&rec.body, a)
        } else {
            (rec.body.clone(), String::new())
        };
        for line in body.lines() {
            println!("  {line}");
        }
        if !footer.is_empty() {
            println!("  {footer}");
        }
    }
    if let Some(promo) = &rec.promotion {
        println!("  [晋升段]");
        for line in promo.lines() {
            println!("  {line}");
        }
    }
    if !rec.devs.is_empty() && (a.has("verbose") || a.has("deviations")) {
        println!("  [解析纠偏 {} 处]", rec.devs.len());
        for d in &rec.devs {
            println!("    {}", d.show());
        }
    }
    Ok(0)
}

/// `history` — commits touching a record, newest first, paged.
pub fn cmd_history(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let needle = a.pos.first().cloned().unwrap_or_default();
    let dir = root.join("commits");
    let mut rows: Vec<(String, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            if !needle.is_empty() {
                let txt = read_str(&p).unwrap_or_default();
                if !txt.contains(&needle) && !name.contains(&needle) {
                    continue;
                }
            }
            let first = read_str(&p)
                .unwrap_or_default()
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim_start_matches('#')
                .trim()
                .to_string();
            rows.push((name, first));
        }
    }
    rows.sort();
    rows.reverse();
    let pg = Page::from_args(a, rows.len());
    for (n, t) in rows.iter().skip(pg.start()).take(pg.per.min(rows.len())) {
        println!("{n}  {t}");
    }
    println!("{}", pg.bar());
    Ok(0)
}

/// `diff <a> <b>` — field-level difference between two records.
pub fn cmd_diff(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    if a.pos.len() < 2 {
        return usage("diff 需要两条记录");
    }
    let (p1, l1) = resolve_record(&root, &a.pos[0])?;
    let (p2, l2) = resolve_record(&root, &a.pos[1])?;
    let r1 = Record::parse(&p1, l1)?;
    let r2 = Record::parse(&p2, l2)?;
    let mut keys: Vec<String> = r1.fields.iter().map(|(k, _)| k.clone()).collect();
    for (k, _) in &r2.fields {
        if !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    let mut n = 0;
    for k in &keys {
        let v1 = r1.get(k);
        let v2 = r2.get(k);
        if v1 != v2 {
            n += 1;
            println!("~ {k}");
            if !v1.is_empty() {
                println!("  - {v1}");
            }
            if !v2.is_empty() {
                println!("  + {v2}");
            }
        }
    }
    if r1.body != r2.body {
        n += 1;
        println!("~ 正文  {} 字节 → {} 字节", r1.body.len(), r2.body.len());
    }
    if n == 0 {
        println!("无差异");
    } else {
        println!("共 {n} 处差异");
    }
    Ok(0)
}

/// `export` — dump records in a machine-readable form, paged.
pub fn cmd_export(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let lib = match a.get("lib").unwrap_or("both") {
        "canonical" | "c" => Some(Lib::Canonical),
        "candidate" | "n" => Some(Lib::Candidate),
        _ => None,
    };
    let fmt = a.get("format").unwrap_or("jsonl").to_string();
    let mut out: Vec<String> = Vec::new();
    let libs: Vec<Lib> = match lib {
        Some(l) => vec![l],
        None => vec![Lib::Canonical, Lib::Candidate],
    };
    for l in libs {
        for rec in load_lib(&root, l) {
            if rec.status() == "DELETED" && !a.has("include-deleted") {
                continue;
            }
            if fmt == "jsonl" {
                let fields = rec
                    .fields
                    .iter()
                    .map(|(k, v)| format!("{}:{}", jstr(k), jstr(v)))
                    .collect::<Vec<_>>()
                    .join(",");
                out.push(format!(
                    "{{\"path\":{},\"lib\":{},\"fields\":{{{fields}}},\"body\":{}}}",
                    jstr(&rel(&root, &rec.path)),
                    jstr(l.name()),
                    jstr(&rec.body)
                ));
            } else {
                out.push(rec.render());
            }
        }
    }
    if let Some(dest) = a.get("out") {
        let dp = expand_tilde(dest);
        let text = out.join(if fmt == "jsonl" { "\n" } else { "\n\n" });
        write_atomic(&dp, &text)?;
        println!("{} 条 → {}", out.len(), dp.display());
        return Ok(0);
    }
    let pg = Page::from_args(a, out.len());
    for line in out.iter().skip(pg.start()).take(pg.per.min(out.len())) {
        println!("{line}");
    }
    if pg.sliced() {
        println!("{}", pg.bar());
    }
    Ok(0)
}

/// `tree` — branch -> record counts, paged.
pub fn cmd_tree(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    for lib in [Lib::Canonical, Lib::Candidate] {
        let ap = root.join(lib.atlas());
        if !ap.exists() {
            continue;
        }
        let at = crate::atlas::Atlas::load(&ap)?;
        println!("== {}", lib.name());
        let mut rows: Vec<(String, usize, String)> = at
            .branches
            .iter()
            .map(|b| (b.name(), b.count(), b.get("STATUS")))
            .collect();
        rows.sort_by(|x, y| y.1.cmp(&x.1));
        let pg = Page::from_args(a, rows.len());
        for (name, n, st) in rows.iter().skip(pg.start()).take(pg.per.min(rows.len())) {
            println!("  {name:<28} {n:>4}  {st}");
        }
        println!("  {}", pg.bar());
    }
    Ok(0)
}

/// `search` — high-performance content search with paging.
pub fn cmd_search(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let mut patterns: Vec<String> = Vec::new();
    let mut i = 0;
    while let Some(p) = a.pos.get(i) {
        patterns.push(p.clone());
        i += 1;
    }
    if let Some(extra) = a.get("and") {
        for part in extra.split(['|', '，', ',']) {
            let t = part.trim();
            if !t.is_empty() {
                patterns.push(t.to_string());
            }
        }
    }
    if patterns.is_empty() {
        return usage("用法: abook search <词> [更多词]  [--regex|--fuzzy|--literal]");
    }
    if a.has("semantic") || a.has("sem") || a.has("ask") {
        return semantic_search(a, &root, patterns);
    }
    // Unified cost tiers. The model picks how much it is willing to pay:
    //   L1 literal   byte prefilter + parallel scan, no decode of misses
    //   L2 fuzzy     bounded edit distance, CPU-heavy per candidate
    //   L3 semantic  in-memory BM25, no expansion
    //   L4 +expand   BM25 plus association expansion (default semantic)
    //   L5 deep      two expansion rounds over a wider seed set
    let level = a.num("level").unwrap_or(0) as u8;
    if level >= 3 && !patterns.is_empty() {
        return semantic_search(a, &root, patterns);
    }
    let mode = if a.has("regex") || a.has("re") {
        crate::search::Mode::Regex
    } else if a.has("fuzzy") || a.has("approx") || level == 2 {
        crate::search::Mode::Fuzzy
    } else if a.has("literal") || a.has("exact") || level == 1 {
        crate::search::Mode::Literal
    } else {
        crate::search::Mode::All
    };
    let opts = crate::search::Opts {
        patterns,
        mode,
        ci: !a.has("case"),
        fuzzy_k: a.num("distance").unwrap_or(1) as usize,
        fields: a
            .get("field")
            .map(|f| f.split(',').map(|x| x.trim().to_string()).collect())
            .unwrap_or_default(),
        body_only: a.get("field").map(|f| f == "body").unwrap_or(false),
        context: a.num("context").unwrap_or(0) as usize,
        max_hits: a.num("max").unwrap_or(0) as usize,
        min_score: a.num("min-score").unwrap_or(0) as i32,
        raw_lines: a.has("raw-lines") || a.has("lines") || a.has("whole-line"),
        lib: match a.get("lib").unwrap_or("") {
            "canonical" | "c" => Some(crate::rec::Lib::Canonical),
            "candidate" | "n" => Some(crate::rec::Lib::Candidate),
            _ => None,
        },
        include_deleted: a.has("include-deleted"),
        include_promoted: !a.has("hide-promoted"),
    };
    let t0 = std::time::Instant::now();
    let hits = match crate::search::search(&root, &opts) {
        Ok(h) => h,
        Err(e) => return usage(e),
    };
    let ms = t0.elapsed().as_millis();
    let pg = Page::from_args(a, hits.len());
    if a.has("json") {
        let rows = hits
            .iter()
            .skip(pg.start())
            .take(pg.per.min(hits.len()))
            .map(|h| {
                format!(
                    "{{\"path\":{},\"score\":{},\"field\":{},\"line\":{},\"title\":{},\"snippet\":{}}}",
                    jstr(&h.path),
                    h.score,
                    jstr(&h.where_),
                    h.line,
                    jstr(&h.title),
                    jstr(&h.snippet)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"total\":{},\"ms\":{},\"results\":[{rows}]}}",
            hits.len(),
            ms
        );
        return Ok(if hits.is_empty() { 3 } else { 0 });
    }
    for h in hits.iter().skip(pg.start()).take(pg.per.min(hits.len())) {
        println!("{:>4}  {}  [{}] {}", h.score, h.path, h.where_, truncate_chars(&h.title, 60));
        if !h.snippet.is_empty() {
            println!("       行{}  {}", h.line, h.snippet);
        }
    }
    println!("{}  {}ms", pg.bar(), ms);
    if hits.is_empty() {
        return Ok(3);
    }
    Ok(0)
}

/// `search --semantic` — BM25 + association expansion, rebuilt in memory.
fn semantic_search(a: &Args, root: &Path, patterns: Vec<String>) -> R<i32> {
    let q = patterns.join(" ");
    let libs: Vec<Lib> = match a.get("lib").unwrap_or("") {
        "canonical" | "c" => vec![Lib::Canonical],
        "candidate" | "n" => vec![Lib::Candidate],
        _ => vec![Lib::Canonical, Lib::Candidate],
    };
    let t0 = std::time::Instant::now();
    let idx = crate::semantic::Index::build(root, &libs);
    let build_ms = t0.elapsed().as_millis();
    let t1 = std::time::Instant::now();
    // Depth follows the requested tier unless overridden explicitly.
    let level = a.num("level").unwrap_or(0) as u8;
    let depth: u8 = if a.has("deep") || level >= 5 {
        2
    } else if a.has("no-expand") || level == 3 {
        0
    } else {
        1
    };
    let rows = idx.query_depth(&q, depth, a.num("max").unwrap_or(20) as usize);
    let q_ms = t1.elapsed().as_millis();
    let pg = Page::from_args(a, rows.len());
    if a.has("json") {
        let body = rows
            .iter()
            .skip(pg.start())
            .take(pg.per.min(rows.len()))
            .map(|(i, sc, why)| {
                let d = &idx.docs[*i];
                format!(
                    "{{\"path\":{},\"score\":{:.4},\"title\":{},\"via\":{}}}",
                    jstr(&d.path),
                    sc,
                    jstr(&d.title),
                    jstr(why)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"query\":{},\"total\":{},\"terms\":{},\"build_ms\":{},\"query_ms\":{},\"results\":[{body}]}}",
            jstr(&q),
            rows.len(),
            idx.total_terms,
            build_ms,
            q_ms
        );
        return Ok(if rows.is_empty() { 3 } else { 0 });
    }
    for (i, sc, why) in rows.iter().skip(pg.start()).take(pg.per.min(rows.len())) {
        let d = &idx.docs[*i];
        println!(
            "{:>7.3}  {}  [{}] {}",
            sc,
            d.path,
            d.lib.name(),
            truncate_chars(&d.title, 64)
        );
        if !why.is_empty() {
            println!("         {why}");
        }
    }
    println!(
        "{}  索引 {} 项 / 建 {}ms / 查 {}ms{}",
        pg.bar(),
        idx.total_terms,
        build_ms,
        q_ms,
        match depth {
            0 => "（纯词频）",
            1 => "（含关联扩展）",
            _ => "（深度扩展）",
        }
    );
    if rows.is_empty() {
        return Ok(3);
    }
    Ok(0)
}

/// `grep` — line-oriented search over raw record text.
pub fn cmd_grep(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let pat = a.need_pos(0, "搜索文本")?;
    let ctx = a.num("context").unwrap_or(0) as usize;
    let mut hits: Vec<String> = Vec::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        for rec in load_lib(&root, lib) {
            let lines: Vec<&str> = rec.raw.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if line.to_lowercase().contains(&pat.to_lowercase()) {
                    hits.push(format!(
                        "{}:{}: {}",
                        rel(&root, &rec.path),
                        i + 1,
                        truncate_chars(line, 160)
                    ));
                    if ctx > 0 {
                        let lo = i.saturating_sub(ctx);
                        let hi = (i + ctx + 1).min(lines.len());
                        for j in lo..hi {
                            if j != i {
                                hits.push(format!("    {}  {}", j + 1, truncate_chars(lines[j], 140)));
                            }
                        }
                    }
                }
            }
        }
    }
    let pg = Page::from_args(a, hits.len());
    for h in hits.iter().skip(pg.start()).take(pg.per.min(hits.len())) {
        println!("{h}");
    }
    println!("{}", pg.bar());
    if hits.is_empty() {
        return Ok(3);
    }
    Ok(0)
}

/// `list` — filtered, paged record listing.
pub fn cmd_list(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let f = ListFilter::from_args(a);
    let mut rows: Vec<Record> = Vec::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        for rec in load_lib(&root, lib) {
            if f.accept(&rec, a) {
                rows.push(rec);
            }
        }
    }
    rows.sort_by(|x, y| y.file_name().cmp(&x.file_name()));
    let pg = Page::from_args(a, rows.len());
    for rec in rows.iter().skip(pg.start()).take(pg.per.min(rows.len())) {
        println!(
            "{}  [{}] {}",
            rel(&root, &rec.path),
            rec.lib.name(),
            truncate_chars(&rec.title(), 70)
        );
    }
    println!("{}", pg.bar());
    Ok(0)
}

/// `stats` with paging.
pub fn cmd_stats(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let mut by_kind: Vec<(String, usize)> = Vec::new();
    let mut by_mat: Vec<(String, usize)> = Vec::new();
    let mut by_status: Vec<(String, usize)> = Vec::new();
    let mut total = 0usize;
    let mut bytes = 0usize;
    let mut devs = 0usize;
    for lib in [Lib::Canonical, Lib::Candidate] {
        for rec in load_lib(&root, lib) {
            if rec.status() == "DELETED" {
                continue;
            }
            total += 1;
            bytes += rec.raw.len();
            devs += rec.devs.len();
            let bump = |v: &mut Vec<(String, usize)>, k: String| {
                if k.is_empty() {
                    return;
                }
                if let Some(e) = v.iter_mut().find(|(x, _)| *x == k) {
                    e.1 += 1;
                } else {
                    v.push((k, 1));
                }
            };
            bump(&mut by_kind, rec.kind());
            bump(&mut by_mat, rec.get("MATURITY"));
            bump(&mut by_status, rec.status());
        }
    }
    println!("记录 {total} 条 / {bytes} 字节 / 解析纠偏 {devs} 处");
    for (title, rows) in [
        ("KIND", &mut by_kind),
        ("MATURITY", &mut by_mat),
        ("STATUS", &mut by_status),
    ] {
        rows.sort_by(|x, y| y.1.cmp(&x.1));
        let pg = Page::from_args(a, rows.len());
        println!("== {title}");
        for (k, n) in rows.iter().skip(pg.start()).take(pg.per.min(rows.len())) {
            println!("  {k:<20} {n}");
        }
    }
    Ok(0)
}

/// `budget` — estimate what a recall would cost, mirroring the prompt's read
/// budget rule so the model can check before it reads.
pub fn cmd_budget(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let preload = [
        "Global.md",
        "Index.md",
        "canonical/Atlas.md",
        "candidate/Atlas.md",
    ];
    let mut total = 0usize;
    println!("预载预算");
    for f in preload {
        let p = root.join(f);
        let n = std::fs::metadata(&p).map(|m| m.len() as usize).unwrap_or(0);
        total += n;
        println!("  {f:<24} {n:>7} B");
    }
    println!("  合计 {total} B（规则上限 12288 B: {}）", if total <= 12288 { "通过" } else { "超限" });
    if let Some(sel) = a.pos.first() {
        let (path, lib) = resolve_record(&root, sel)?;
        let rec = Record::parse(&path, lib)?;
        println!("单条 {}  {} B（规则上限 8192 B: {}）",
            rel(&root, &path),
            rec.raw.len(),
            if rec.raw.len() <= 8192 { "通过" } else { "超限" });
    }
    Ok(0)
}
