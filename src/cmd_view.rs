//! Viewing: paged listings, single-field read, history, diff, export, tree.

use crate::paging::Page;
use crate::rec::{load_lib, resolve_record, ListFilter, Lib, Record};
use crate::util::*;
use std::path::{Path, PathBuf};

/// `get <record> <field>` — read one field without dumping the whole record.
pub fn cmd_get(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let (path, lib) = resolve_record(&root, sel)?;
    let rec = Record::parse(&path, lib)?;
    let want_json = a.has("json");
    // `--field` was listed in the parameter table but the code only ever read
    // the second positional, so `get X --field TITLE` printed every field name
    // instead of the title. Both spellings are honoured now.
    let field_arg: Option<String> = a
        .get("field")
        .map(|s| s.to_string())
        .or_else(|| a.pos.get(1).cloned());
    match field_arg.as_ref() {
        Some(f) => {
            let (canon, _) = crate::parse::canon_key(f);
            if canon == "BODY" || canon == "MEMORY_BODY" {
                if want_json {
                    println!(
                        "{{\"id\":{},\"path\":{},\"field\":\"BODY\",\"value\":{}}}",
                        jstr(&rec.id()),
                        jstr(&rel(&root, &path)),
                        jstr(&rec.body)
                    );
                } else {
                    println!("{}", rec.body);
                }
                return Ok(0);
            }
            if !rec.has(&canon) {
                return notfound(format!("记录没有字段 {canon}"));
            }
            if want_json {
                println!(
                    "{{\"id\":{},\"path\":{},\"field\":{},\"value\":{}}}",
                    jstr(&rec.id()),
                    jstr(&rel(&root, &path)),
                    jstr(&canon),
                    jstr(&rec.get(&canon))
                );
            } else {
                println!("{}", rec.get(&canon));
            }
            Ok(0)
        }
        None => {
            if want_json {
                let mut items: Vec<String> = Vec::new();
                for (k, v) in &rec.fields {
                    items.push(format!(
                        "{{\"field\":{},\"chars\":{},\"preview\":{}}}",
                        jstr(k),
                        v.chars().count(),
                        jstr(&truncate_chars(v, 60))
                    ));
                }
                items.push(format!(
                    "{{\"field\":\"BODY\",\"chars\":{},\"preview\":{}}}",
                    rec.body.chars().count(),
                    jstr(&truncate_chars(&rec.body, 60))
                ));
                println!(
                    "{{\"id\":{},\"path\":{},\"fields\":[{}]}}",
                    jstr(&rec.id()),
                    jstr(&rel(&root, &path)),
                    items.join(",")
                );
                return Ok(0);
            }
            // no field: list the field names only
            for (k, v) in &rec.fields {
                println!("{k}\t{}\t{}", v.chars().count(), truncate_chars(v, 60));
            }
            println!("BODY\t{}\t{}", rec.body.chars().count(), truncate_chars(&rec.body, 60));
            Ok(0)
        }
    }
}

/// Shared lister for the library's note directories.
///
/// `commits`/`review`/`staging` hold real files with real content — the live
/// library has 107 commits, 21 review entries and 1 staging item — but only
/// `commits` was readable (through `history`). `paths` printed the other two
/// as though they were part of the tool's vocabulary while nothing could open
/// them, so an agent knew they existed and still could not read them. One
/// implementation here means adding a future directory is a single line.
fn note_dir(
    a: &Args,
    dir: &str,
    noun: &str,
    json_key: &str,
) -> R<i32> {
    let root = resolve_root(a);
    let d = root.join(dir);
    let needle = a.pos.first().cloned().unwrap_or_default();
    // `--show` opens one file whole; the positional argument filters the list.
    if let Some(name) = a.get("show") {
        let p = if Path::new(name).is_absolute() {
            PathBuf::from(name)
        } else {
            d.join(name)
        };
        if !p.is_file() {
            return usage(&format!("{dir} 里没有 `{name}`；先运行 `abook {dir}` 看有哪些"));
        }
        let txt = read_str(&p)?;
        if a.has("json") {
            println!(
                "{{\"name\":{},\"path\":{},\"bytes\":{},\"body\":{}}}",
                jstr(name),
                jstr(&p.display().to_string()),
                txt.len(),
                jstr(&txt)
            );
        } else {
            print!("{txt}");
            if !txt.ends_with('\n') {
                println!();
            }
        }
        return Ok(0);
    }
    let mut rows: Vec<(String, String, u64)> = Vec::new();
    // Subdirectories are listed as rows, not skipped. `review/` really does
    // contain `abandoned-20260928/`, and silently dropping it made the count
    // disagree with `ls` while looking perfectly healthy -- the same
    // disappearance bug this tool exists to stop.
    let mut subdirs: Vec<(String, usize)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&d) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let n = std::fs::read_dir(&p).map(|r| r.flatten().count()).unwrap_or(0);
                let name = p
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                subdirs.push((name, n));
                continue;
            }
            if !p.is_file() {
                continue;
            }
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let txt = read_str(&p).unwrap_or_default();
            if !needle.is_empty() && !txt.contains(&needle) && !name.contains(&needle) {
                continue;
            }
            let title = txt
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim_start_matches('#')
                .trim()
                .to_string();
            rows.push((name, title, txt.len() as u64));
        }
    }
    rows.sort();
    rows.reverse();
    let pg = Page::from_args(a, rows.len());
    if a.has("json") {
        let items = rows
            .iter()
            .skip(pg.start())
            .take(pg.per.min(rows.len()))
            .map(|(n, t, b)| {
                format!(
                    "{{\"name\":{},\"title\":{},\"bytes\":{}}}",
                    jstr(n),
                    jstr(t),
                    b
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!("{{{},\"{json_key}\":[{items}]}}", pg.json_fields());
        return Ok(0);
    }
    for (n, t, b) in rows.iter().skip(pg.start()).take(pg.per.min(rows.len())) {
        println!("{n:<56} {:>7}  {t}", human_bytes(*b));
    }
    println!("{}", pg.bar());
    let sub = if subdirs.is_empty() {
        String::new()
    } else {
        let names = subdirs
            .iter()
            .map(|(n, c)| format!("{n}/（{c} 个文件）"))
            .collect::<Vec<_>>()
            .join("、");
        format!("；另有子目录 {names}，需直接查看")
    };
    println!("共 {} 条{noun}{sub}", rows.len());
    if !a.has("json") {
        println!("看全文: abook {dir} --show <文件名>");
    } else {
        // Machine callers must see the subdirectories too, or they will
        // conclude the directory holds fewer entries than it does.
        let s2 = subdirs
            .iter()
            .map(|(n, c)| format!("{{\"name\":{},\"files\":{c}}}", jstr(n)))
            .collect::<Vec<_>>()
            .join(",");
        eprintln!("{{\"subdirs\":[{s2}]}}");
    }
    Ok(0)
}

/// `staging` — pending decision items awaiting a human answer.
pub fn cmd_staging(a: &Args) -> R<i32> {
    note_dir(a, "staging", "暂存", "staging")
}

/// `review` — promotion proposals and their outcomes.
pub fn cmd_review(a: &Args) -> R<i32> {
    note_dir(a, "review", "晋升记录", "review")
}

/// `history` — commits touching a record, newest first, paged.
pub fn cmd_history(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let needle = a.pos.first().cloned().unwrap_or_default();
    let dir = root.join("commits");
    // Sort by the commit's own `TIME=` field, not by filename.
    //
    // The live library mixes two filename schemes: the old `0001-...` sequence
    // and the current `<stamp>-...`. Sorting names put the sequence after the
    // timestamps, and within one timestamp put `-1` before the bare name --
    // 44 of 76 rows landed in the wrong position. The file already records
    // when it happened, so ask the file.
    let mut rows: Vec<(i64, String, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let txt = read_str(&p).unwrap_or_default();
            if !needle.is_empty() && !txt.contains(&needle) && !name.contains(&needle) {
                continue;
            }
            let stamp = commit_time(&p, &txt);
            let first = txt
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim_start_matches('#')
                .trim()
                .to_string();
            rows.push((stamp, name, first));
        }
    }
    // Newest first; the name is the tie-break so equal timestamps stay stable.
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let pg = Page::from_args(a, rows.len());
    if a.has("json") {
        let items = rows
            .iter()
            .skip(pg.start())
            .take(pg.per.min(rows.len()))
            .map(|(_, n, t)| format!("{{\"name\":{},\"title\":{}}}", jstr(n), jstr(t)))
            .collect::<Vec<_>>()
            .join(",");
        println!("{{{},\"commits\":[{items}]}}", pg.json_fields());
        return Ok(0);
    }
    for (_, n, t) in rows.iter().skip(pg.start()).take(pg.per.min(rows.len())) {
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
    let mut changes: Vec<(String, String, String)> = Vec::new();
    for k in &keys {
        let v1 = r1.get(k);
        let v2 = r2.get(k);
        if v1 != v2 {
            changes.push((k.clone(), v1, v2));
        }
    }
    let body_changed = r1.body != r2.body;
    let n = changes.len() + usize::from(body_changed);
    if a.has("json") {
        let items = changes
            .iter()
            .map(|(k, a1, b1)| {
                format!(
                    "{{\"field\":{},\"left\":{},\"right\":{}}}",
                    jstr(k),
                    jstr(a1),
                    jstr(b1)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"left\":{},\"right\":{},\"differences\":{n},\"fields\":[{items}],\"body_changed\":{},\"body_bytes\":[{},{}]}}",
            jstr(&r1.id()),
            jstr(&r2.id()),
            body_changed,
            r1.body.len(),
            r2.body.len()
        );
        return Ok(0);
    }
    for (k, v1, v2) in &changes {
        println!("~ {k}");
        if !v1.is_empty() {
            println!("  - {v1}");
        }
        if !v2.is_empty() {
            println!("  + {v2}");
        }
    }
    if body_changed {
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
    // `--format` is a free-form string, so an unrecognised value used to fall
    // through to the markdown branch. Asking for `json` therefore returned raw
    // `[CANONICAL_MEMORY]` blocks: valid text, wrong type, and nothing said so.
    // A format the tool cannot produce is a usage error, not a silent downgrade.
    if !matches!(fmt.as_str(), "jsonl" | "json" | "md" | "markdown") {
        return crate::util::usage(format!(
            "未知导出格式 {fmt}（可选 jsonl / json / md）"
        ));
    }
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
            if fmt == "jsonl" || fmt == "json" {
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
    // `json` is one array, `jsonl` is one document per line. Both carry the same
    // objects; the difference is whether a streaming reader can take a line at
    // a time without buffering the whole export.
    // Joined on a bare comma so the array occupies one line. Multi-line JSON is
    // legal, but the whole point of `--json` here is that a consuming script can
    // treat every line it reads as one complete value; a pretty-printed array
    // breaks that for no benefit.
    let text = if fmt == "json" {
        format!("[{}]", out.join(","))
    } else {
        out.join(if fmt == "jsonl" { "\n" } else { "\n\n" })
    };
    if let Some(dest) = a.get("out") {
        let dp = expand_tilde(dest);
        write_atomic(&dp, &text)?;
        println!("{} 条 → {}", out.len(), dp.display());
        return Ok(0);
    }
    let pg = Page::from_args(a, out.len());
    let page: Vec<&String> = out.iter().skip(pg.start()).take(pg.per.min(out.len())).collect();
    if fmt == "json" {
        println!("[{}]", page.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(","));
    } else {
        for line in page {
            println!("{line}");
        }
    }
    // The paging footer goes to stderr under `--json`.
    //
    // It is prose, and it was landing as the last line of a JSONL stream. A
    // reader that parses line by line got 30 good records and then a
    // "第 1/17 页" line that failed to parse -- with no way to tell a real
    // corruption from the tool's own footer. On stderr it stays available to a
    // human and out of the data.
    if pg.sliced() {
        if a.has("json") {
            eprintln!("{}", pg.bar());
        } else {
            println!("{}", pg.bar());
        }
        // Say where the rest is. A caller that only reads stdout would
        // otherwise silently receive page 1 and believe it was the whole
        // library; the count it needs is also on stderr as a document.
        if a.has("json") {
            eprintln!(
                "{{\"page\":{},\"per_page\":{},\"total\":{},\"pages\":{}}}",
                pg.page, pg.per, pg.total, pg.pages
            );
        }
    }
    Ok(0)
}

/// `tree` — branch -> record counts, paged.
pub fn cmd_tree(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let mut js: Vec<String> = Vec::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        let ap = root.join(lib.atlas());
        if !ap.exists() {
            continue;
        }
        let at = crate::atlas::Atlas::load(&ap)?;
        let mut rows: Vec<(String, usize, String)> = at
            .branches
            .iter()
            .map(|b| (b.name(), b.count(), b.get("STATUS")))
            .collect();
        rows.sort_by(|x, y| y.1.cmp(&x.1));
        let pg = Page::from_args(a, rows.len());
        if a.has("json") {
            let items = rows
                .iter()
                .skip(pg.start())
                .take(pg.per.min(rows.len()))
                .map(|(name, n, st)| {
                    format!(
                        "{{\"branch\":{},\"count\":{n},\"status\":{}}}",
                        jstr(name),
                        jstr(st)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            js.push(format!(
                "{{{},\"lib\":{},\"branches\":[{items}]}}",
                pg.json_fields(),
                jstr(lib.name())
            ));
            continue;
        }
        println!("== {}", lib.name());
        for (name, n, st) in rows.iter().skip(pg.start()).take(pg.per.min(rows.len())) {
            println!("  {name:<28} {n:>4}  {st}");
        }
        println!("  {}", pg.bar());
    }
    if a.has("json") {
        println!("{{\"libs\":[{}]}}", js.join(","));
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
    // `--deep` on its own has to reach the semantic path, not just widen it once
    // it is already there. It used to be read only inside `semantic_search`, so
    // `search 词 --deep` stayed on the literal tier and returned exactly what
    // `search 词` returned -- the most expensive tier silently cost the least and
    // found the least. The flag is the tier here, so it decides the route.
    if a.has("deep") && !patterns.is_empty() {
        return semantic_search(a, &root, patterns);
    }
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
    // Structured hits first, so the text and JSON renderings cannot disagree
    // about what was found or how many.
    let mut found: Vec<(String, usize, String)> = Vec::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        for rec in load_lib(&root, lib) {
            for (i, line) in rec.raw.lines().enumerate() {
                if line.to_lowercase().contains(&pat.to_lowercase()) {
                    found.push((
                        rel(&root, &rec.path),
                        i + 1,
                        truncate_chars(line, 200),
                    ));
                }
            }
        }
    }
    let pg = Page::from_args(a, found.len());
    if a.has("json") {
        let items = found
            .iter()
            .skip(pg.start())
            .take(pg.per.min(found.len()))
            .map(|(p, ln, t)| {
                format!(
                    "{{\"path\":{},\"line\":{ln},\"text\":{}}}",
                    jstr(p),
                    jstr(t)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{{},\"pattern\":{},\"total\":{},\"hits\":[{items}]}}",
            pg.json_page_only(),
            jstr(&pat),
            found.len()
        );
        return Ok(if found.is_empty() { 3 } else { 0 });
    }
    let mut printed = 0usize;
    for (idx, (p, ln, t)) in found.iter().enumerate() {
        if idx < pg.start() || idx >= pg.start() + pg.per {
            continue;
        }
        println!("{p}:{ln}: {t}");
        printed += 1;
        if ctx > 0 {
            if let Ok(txt) = std::fs::read_to_string(root.join(p)) {
                let lines: Vec<&str> = txt.lines().collect();
                let lo = (ln - 1).saturating_sub(ctx);
                let hi = ((ln - 1) + ctx + 1).min(lines.len());
                for j in lo..hi {
                    if j != ln - 1 {
                        println!("    {}  {}", j + 1, truncate_chars(lines[j], 140));
                    }
                }
            }
        }
    }
    let _ = printed;
    println!("{}", pg.bar());
    if found.is_empty() {
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
    if a.has("json") {
        // A caller that asked for JSON and got a table would silently parse
        // nothing, so every command that advertises --json has to branch here.
        let items = rows
            .iter()
            .skip(pg.start())
            .take(pg.per.min(rows.len()))
            .map(|rec| {
                format!(
                    "{{\"path\":{},\"lib\":{},\"id\":{},\"title\":{},\"status\":{},\"kind\":{},\"maturity\":{},\"bytes\":{}}}",
                    jstr(&rel(&root, &rec.path)),
                    jstr(rec.lib.name()),
                    jstr(&rec.id()),
                    jstr(&rec.title()),
                    jstr(&rec.status()),
                    jstr(&rec.kind()),
                    jstr(&rec.get("MATURITY")),
                    rec.raw.len()
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!("{{{},\"records\":[{items}]}}", pg.json_fields());
        return Ok(0);
    }
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
    // Measure once, then render either form: recomputing sizes per branch is
    // how the two outputs drift apart.
    let mut parts: Vec<(String, usize)> = Vec::new();
    for f in preload {
        let p = root.join(f);
        let n = std::fs::metadata(&p).map(|m| m.len() as usize).unwrap_or(0);
        total += n;
        parts.push((f.to_string(), n));
    }
    let one = match a.pos.first() {
        Some(sel) => {
            let (path, lib) = resolve_record(&root, sel)?;
            let rec = Record::parse(&path, lib)?;
            Some((rel(&root, &path), rec.raw.len()))
        }
        None => None,
    };
    if a.has("json") {
        let files = parts
            .iter()
            .map(|(k, v)| format!("{}:{}", jstr(k), v))
            .collect::<Vec<_>>()
            .join(",");
        let rec = match &one {
            Some((p, n)) => format!(
                "{{\"path\":{},\"bytes\":{},\"limit\":8192,\"ok\":{}}}",
                jstr(p),
                n,
                *n <= 8192
            ),
            None => "null".to_string(),
        };
        println!(
            "{{\"files\":{{{files}}},\"total\":{total},\"limit\":12288,\"ok\":{},\"record\":{rec}}}",
            total <= 12288
        );
        return Ok(0);
    }
    println!("预载预算");
    for (f, n) in &parts {
        println!("  {f:<24} {n:>7} B");
    }
    println!("  合计 {total} B（规则上限 12288 B: {}）", if total <= 12288 { "通过" } else { "超限" });
    if let Some((path, n)) = &one {
        println!("单条 {path}  {n} B（规则上限 8192 B: {}）",
            if *n <= 8192 { "通过" } else { "超限" });
    }
    Ok(0)
}
