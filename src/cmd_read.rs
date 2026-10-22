//! Read-only commands: state, stats, recall, search, show, atlas, index,
//! recent, lint, doctor, reconcile.

use crate::atlas::Atlas;
use crate::book::BookFile;
use crate::query::*;
use crate::rec::{load_lib, resolve_record, Lib, Record};
use crate::util::*;
use std::path::Path;

fn libs_from(a: &Args) -> Vec<Lib> {
    let l = a.get("lib").unwrap_or("both");
    match l {
        "canonical" | "c" | "正式" => vec![Lib::Canonical],
        "candidate" | "n" | "候选" => vec![Lib::Candidate],
        _ => vec![Lib::Canonical, Lib::Candidate],
    }
}

pub fn cmd_state(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    if !root.exists() {
        return notfound(format!("记忆根不存在: {}", root.display()));
    }
    let st = read_state(&root);
    if a.has("json") {
        println!(
            "{{\"root\":{},\"canonical\":{},\"candidate\":{},\"canonical_atlas\":{},\
\"candidate_atlas\":{},\"vault_files\":{},\"vault_rows\":{},\"covered_canonical\":{},\
\"covered_candidate\":{},\"latest_commit\":{},\"problems\":[{}]}}",
            jstr(&st.root),
            st.canonical,
            st.candidate,
            st.canonical_atlas_count,
            st.candidate_atlas_count,
            st.vault_files,
            st.vault_rows,
            jstr(&st.covered_canonical),
            jstr(&st.covered_candidate),
            jstr(&st.latest_commit),
            st.problems
                .iter()
                .map(|p| jstr(p))
                .collect::<Vec<_>>()
                .join(",")
        );
        return Ok(0);
    }
    println!("记忆根      {}", st.root);
    println!("正式库      {} 条记录 / 地图 {} 条", st.canonical, st.canonical_atlas_count);
    println!("候选库      {} 条记录 / 地图 {} 条", st.candidate, st.candidate_atlas_count);
    println!("收录时刻    正式 {}  候选 {}", st.covered_canonical, st.covered_candidate);
    println!("最新变更    {}", if st.latest_commit.is_empty() { "-" } else { &st.latest_commit });
    println!("保险箱      {} 文件 / 名册 {} 行", st.vault_files, st.vault_rows);
    if st.problems.is_empty() {
        println!("一致性      通过");
    } else {
        println!("一致性      发现 {} 个问题", st.problems.len());
        for p in &st.problems {
            println!("  - {p}");
        }
    }
    Ok(if st.problems.is_empty() { 0 } else { 5 })
}

pub fn cmd_stats(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    for lib in [Lib::Canonical, Lib::Candidate] {
        let recs = load_lib(&root, lib);
        let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
        let mut mats: std::collections::BTreeMap<String, usize> = Default::default();
        let mut promoted = 0;
        for r in &recs {
            *kinds.entry(r.kind()).or_insert(0) += 1;
            *mats.entry(r.get("MATURITY")).or_insert(0) += 1;
            if r.promotion.is_some() {
                promoted += 1;
            }
        }
        println!("== {}  {} 条", lib.name(), recs.len());
        if lib == Lib::Candidate {
            println!("   已晋升 {}", promoted);
        }
        print!("   KIND:");
        for (k, v) in &kinds {
            print!(" {k}={v}");
        }
        println!();
        print!("   MATURITY:");
        for (k, v) in &mats {
            let kk = if k.is_empty() { "?" } else { k.as_str() };
            print!(" {kk}={v}");
        }
        println!();
        if let Ok(at) = Atlas::load(&root.join(lib.atlas())) {
            println!("   分支 {} 个 / 路径 {} 条", at.branches.len(), at.total_count());
        }
    }
    Ok(0)
}

pub fn cmd_recall(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let q = match a.pos.first() {
        Some(q) => q.clone(),
        None => a.get("q").unwrap_or("").to_string(),
    };
    if q.trim().is_empty() {
        return usage("recall 需要查询词");
    }
    let libs = libs_from(a);
    let limit = a.num("limit").unwrap_or(8) as usize;
    let with_body = a.has("body") || a.has("raw");
    let res = recall(
        &root,
        &q,
        &libs,
        limit,
        a.has("include-promoted"),
        with_body,
    );
    if a.has("json") {
        let arr = res
            .hits
            .iter()
            .map(|h| {
                format!(
                    "{{\"lib\":{},\"path\":{},\"title\":{},\"tier\":{},\"score\":{},\
\"reason\":{},\"body\":{}}}",
                    jstr(h.lib.name()),
                    jstr(&h.path),
                    jstr(&h.title),
                    jstr(h.tier.label()),
                    h.score,
                    jstr(&h.reason),
                    jstr(&h.record.as_ref().map(|r| r.body.clone()).unwrap_or_default())
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"status\":{},\"query\":{},\"scanned\":{},\"hits\":[{}],\"rejected\":{}}}",
            jstr(res.status),
            jstr(&res.query),
            res.scanned,
            arr,
            res.rejected.len()
        );
        return Ok(0);
    }
    println!("{}  「{}」  扫描 {} 条", res.status, res.query, res.scanned);
    if res.hits.is_empty() {
        println!("  无命中。");
    }
    for h in &res.hits {
        println!(
            "  [{}] {} {}  ({} / {})",
            h.lib.name(),
            h.title,
            h.path,
            h.tier.label(),
            h.reason
        );
        if with_body {
            if let Some(r) = &h.record {
                for line in r.body.lines().take(6) {
                    println!("      {line}");
                }
            }
        }
    }
    if !res.rejected.is_empty() && (a.has("verbose") || a.has("all")) {
        println!("  被精度过滤 {} 条:", res.rejected.len());
        for h in &res.rejected {
            println!(
                "    - {} {}  ({})",
                h.title,
                h.path,
                h.rejected.clone().unwrap_or_default()
            );
        }
    }
    Ok(if res.hits.is_empty() { 3 } else { 0 })
}

pub fn cmd_show(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录选择器（路径 / 文件名 / ID）")?;
    let (path, lib) = resolve_record(&root, sel)?;
    let rec = Record::parse(&path, lib)?;
    if a.has("json") {
        println!(
            "{{\"path\":{},\"lib\":{},\"fields\":{{{}}},\"body\":{}}}",
            jstr(&rel(&root, &path)),
            jstr(lib.name()),
            rec.fields
                .iter()
                .map(|(k, v)| format!("{}:{}", jstr(k), jstr(v)))
                .collect::<Vec<_>>()
                .join(","),
            jstr(&rec.body)
        );
        return Ok(0);
    }
    println!("{}  [{}]", rel(&root, &path), lib.name());
    for (k, v) in &rec.fields {
        let vv = if v.chars().count() > 200 && !a.has("verbose") {
            truncate_chars(v, 200)
        } else {
            v.clone()
        };
        println!("  {k}={vv}");
    }
    println!();
    println!("{}", rec.body);
    if let Some(p) = &rec.promotion {
        println!();
        println!("[PROMOTION]");
        println!("{}", p.trim_end());
    }
    Ok(0)
}

pub fn cmd_search(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let pat = a.need_pos(0, "搜索模式（支持 * 与 ?）")?;
    let libs = libs_from(a);
    let mut n = 0;
    for lib in &libs {
        for rec in load_lib(&root, *lib) {
            let hay = format!(
                "{} {} {} {}",
                rec.title(),
                rec.get("KEYS"),
                rec.get("CONCEPTS"),
                rec.body
            );
            let hit = if pat.contains('*') || pat.contains('?') {
                let base = rec.file_name();
                wildcard_match(pat, &base) || wildcard_match(&format!("*{pat}*"), &hay)
            } else {
                ci_contains(&hay, pat) || ci_contains(&rec.file_name(), pat)
            };
            if hit {
                n += 1;
                println!("[{}] {}  {}", lib.name(), rec.title(), rel(&root, &rec.path));
                if a.has("verbose") {
                    for line in rec.body.lines().take(3) {
                        println!("    {line}");
                    }
                }
            }
        }
    }
    if n == 0 {
        println!("无命中");
        return Ok(3);
    }
    println!("共 {n} 条");
    Ok(0)
}

pub fn cmd_atlas(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let which = a.pos_or(0, "both");
    let libs: Vec<Lib> = match which.as_str() {
        "canonical" | "c" => vec![Lib::Canonical],
        "candidate" | "n" => vec![Lib::Candidate],
        _ => vec![Lib::Canonical, Lib::Candidate],
    };
    for lib in libs {
        let p = root.join(lib.atlas());
        if !p.exists() {
            println!("{} 不存在", lib.atlas());
            continue;
        }
        let at = Atlas::load(&p)?;
        println!(
            "== {}  {} 分支 / {} 路径  COVERED_THROUGH={}",
            lib.atlas(),
            at.branches.len(),
            at.total_count(),
            at.get("COVERED_THROUGH")
        );
        if let Some(want) = a.get("card") {
            for b in &at.branches {
                if b.name() == want || b.heading == want {
                    println!("== 卡片 {}", b.name());
                    for (k, v) in &b.fields {
                        println!("  {k}={}", truncate_chars(v, 300));
                    }
                    println!("  [解析] PATH {} 条", b.paths().len());
                }
            }
            continue;
        }
        for b in &at.branches {
            if a.has("verbose") {
                println!(
                    "   {:<24} 声明={:<4} 实析={:<4} PATH原文={}",
                    b.name(),
                    b.get("COUNT"),
                    b.count(),
                    b.paths().len()
                );
            } else {
                println!(
                    "   {:<24} COUNT={:<4} REVISION={:<3} {}",
                    b.name(),
                    b.get("COUNT"),
                    b.get("REVISION"),
                    truncate_chars(&b.get("KEYS"), 60)
                );
            }
        }
    }
    Ok(0)
}

pub fn cmd_index(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let p = root.join("Index.md");
    if !p.exists() {
        return notfound("Index.md 不存在");
    }
    let bf = BookFile::load(&p)?;
    let routes = bf.routes();
    println!("Index.md  路由 {} 条", routes.len());
    let q = a.pos.first().cloned().unwrap_or_default();
    let mut shown = 0;
    for (path, summary, when) in &routes {
        if !q.is_empty()
            && !(ci_contains(path, &q) || ci_contains(summary, &q) || ci_contains(when, &q))
        {
            continue;
        }
        shown += 1;
        println!("  {path}｜{summary}｜{when}");
        if shown >= a.num("limit").unwrap_or(40) as usize {
            println!("  …");
            break;
        }
    }
    Ok(0)
}

pub fn cmd_lint(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let libs = libs_from(a);
    let mut bad = 0;
    let mut checked = 0;
    for lib in &libs {
        for rec in load_lib(&root, *lib) {
            checked += 1;
            let probs = rec.lint();
            if !probs.is_empty() {
                bad += 1;
                println!("[{}] {}  {}", lib.name(), rec.file_name(), rec.title());
                for pr in &probs {
                    println!("    - {pr}");
                }
            }
        }
    }
    println!("检查 {checked} 条，问题 {bad} 条");
    Ok(if bad > 0 { 5 } else { 0 })
}

pub fn cmd_doctor(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let mut fail = 0;
    println!("记忆根    {}", root.display());
    if !root.exists() {
        println!("  缺失：请先运行 abook init");
        return Ok(4);
    }
    for f in ["Global.md", "Index.md"] {
        let p = root.join(f);
        if p.exists() {
            let n = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            println!("  {f:<12} 存在 {}", human_bytes(n));
        } else {
            println!("  {f:<12} 缺失");
            fail += 1;
        }
    }
    for lib in [Lib::Canonical, Lib::Candidate] {
        let d = root.join(lib.dir());
        let n = list_md(&d).len();
        println!("  {:<20} {} 条", lib.dir(), n);
        if !root.join(lib.atlas()).exists() {
            println!("  {:<20} 地图缺失", lib.atlas());
            fail += 1;
        }
    }
    let st = read_state(&root);
    for p in &st.problems {
        println!("  问题: {p}");
        fail += 1;
    }
    if a.has("fix") {
        for lib in [Lib::Canonical, Lib::Candidate] {
            let r = crate::cmd_admin::refresh_atlas(&root, lib, false)?;
            println!("  已刷新 {}: {} 条路径", lib.atlas(), r);
        }
        let _ = crate::vault::sync_ledger(&root)?;
        println!("  已同步名册");
        return Ok(0);
    }
    if fail == 0 {
        println!("结论      健康");
    } else {
        println!("结论      {fail} 个问题（可用 --fix 修复可自动处理项）");
    }
    Ok(if fail > 0 { 5 } else { 0 })
}

pub fn cmd_reconcile(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let mut problems = 0;
    for lib in [Lib::Canonical, Lib::Candidate] {
        let recs = load_lib(&root, lib);
        let disk: Vec<String> = list_md(&root.join(lib.dir()))
            .iter()
            .map(|p| rel(&root, p))
            .collect();
        let at = Atlas::load(&root.join(lib.atlas()))?;
        let in_map = at.all_paths();
        let mut missing: Vec<String> = Vec::new();
        for d in &disk {
            let base = d.rsplit('/').next().unwrap_or(d);
            if !in_map.iter().any(|m| m.rsplit('/').next().unwrap_or(m) == base) {
                missing.push(d.clone());
            }
        }
        let mut ghost: Vec<String> = Vec::new();
        for m in &in_map {
            let base = m.rsplit('/').next().unwrap_or(m);
            if !disk.iter().any(|d| d.rsplit('/').next().unwrap_or(d) == base) {
                ghost.push(m.clone());
            }
        }
        println!(
            "== {}  目录 {} / 地图 {} / 已读 {}",
            lib.name(),
            disk.len(),
            in_map.len(),
            recs.len()
        );
        if !missing.is_empty() {
            println!("   地图未收录 {} 条:", missing.len());
            for m in missing.iter().take(10) {
                println!("     + {m}");
            }
            problems += missing.len();
        }
        if !ghost.is_empty() {
            println!("   地图有幽灵条目 {} 条:", ghost.len());
            for g in ghost.iter().take(10) {
                println!("     - {g}");
            }
            problems += ghost.len();
        }
        if missing.is_empty() && ghost.is_empty() {
            println!("   对账一致");
        }
    }
    let (f, r, vp) = crate::vault::reconcile(&root);
    println!("== 保险箱  文件 {f} / 名册 {r}");
    for p in &vp {
        println!("   - {p}");
        problems += 1;
    }
    Ok(if problems > 0 { 5 } else { 0 })
}

pub fn cmd_recent(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let dir = root.join("commits");
    let mut fs: Vec<(std::time::SystemTime, String, u64)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let m = std::fs::metadata(&p).ok();
            let mt = m
                .as_ref()
                .and_then(|x| x.modified().ok())
                .unwrap_or(std::time::UNIX_EPOCH);
            let sz = m.map(|x| x.len()).unwrap_or(0);
            fs.push((
                mt,
                p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                sz,
            ));
        }
    }
    fs.sort_by(|a, b| b.0.cmp(&a.0));
    let n = a.num("limit").unwrap_or(10) as usize;
    for (i, (_, name, sz)) in fs.iter().take(n).enumerate() {
        println!("{:>2}. {:<52} {}", i + 1, name, human_bytes(*sz));
    }
    println!("共 {} 条变更记录", fs.len());
    Ok(0)
}

pub fn cmd_paths(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    println!("Global     {}", root.join("Global.md").display());
    println!("Index      {}", root.join("Index.md").display());
    println!("正式库     {}", root.join(Lib::Canonical.dir()).display());
    println!("候选库     {}", root.join(Lib::Candidate.dir()).display());
    println!("正式图     {}", root.join(Lib::Canonical.atlas()).display());
    println!("候选图     {}", root.join(Lib::Candidate.atlas()).display());
    println!("变更记录   {}", root.join("commits").display());
    println!("晋升记录   {}", root.join("review").display());
    println!("暂存       {}", root.join("staging").display());
    println!("保险箱     {}", root.join("vault").display());
    Ok(0)
}

/// Utility used by cmd_admin for branch listing.
pub fn branch_names(root: &Path, lib: Lib) -> Vec<String> {
    Atlas::load(&root.join(lib.atlas()))
        .map(|a| a.branches.iter().map(|b| b.name()).collect())
        .unwrap_or_default()
}
