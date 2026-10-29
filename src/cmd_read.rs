//! Read-only commands: state, stats, recall, search, show, atlas, index,
//! recent, lint, doctor, reconcile.

use crate::atlas::Atlas;
use crate::book::BookFile;
use crate::cmd_view::{count_lines, count_lines_bytes, count_lines_from, jopt, meta_json};
use crate::query::*;
use crate::paging::Page;
use crate::rec::{load_lib, resolve_record, Lib, Record};
use crate::util::*;
use std::path::Path;


pub fn cmd_state(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    if !root.exists() {
        return notfound(
            Fail::new(format!("记忆根不存在: {}", root.display()))
                .hint("新建一个空库用 `abook init --root <路径>`；如果库在别处，把正确的根传进来（或把 abook 放到那个目录里）")
                .render(),
        );
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
    // One JSON object per library stacked on separate lines is JSONL, not JSON:
    // json.loads() rejects it with "Extra data". Collect and emit one array.
    let mut json_rows: Vec<String> = Vec::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        let recs = load_lib(&root, lib);
        let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
        let mut mats: std::collections::BTreeMap<String, usize> = Default::default();
        let mut promoted = 0;
        for r in &recs {
            *kinds.entry(r.kind()).or_insert(0) += 1;
            *mats.entry(r.get("MATURITY")).or_insert(0) += 1;
            // 用记录自己的判据，不要在这里再写一遍"怎么算已晋升"。
            //
            // 原来这里只看末尾 `[PROMOTION]` 块，于是早期流程把 `PROMOTED_TO`
            // 直接写在头部的那批老记录一条都数不到：实测真实库 301 条候选里
            // `list --status PROMOTED` 报 299 条、这里报 292 条，同一份数据两个
            // 数字差 7 条，而它俩本该永远相等——差的那几条正是最早一批晋升的，
            // 那时还没有末尾块这套写法。
            if r.is_promoted() {
                promoted += 1;
            }
        }
        if a.has("json") {
            let dump = |m: &std::collections::BTreeMap<String, usize>| -> String {
                m.iter()
                    .map(|(k, v)| format!("{}:{}", jstr(if k.is_empty() { "?" } else { k.as_str() }), v))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let (branches, paths) = match Atlas::load(&root.join(lib.atlas())) {
                Ok(at) => (at.branches.len(), at.total_count()),
                Err(_) => (0, 0),
            };
            json_rows.push(format!(
                "{{\"lib\":{},\"records\":{},\"promoted\":{},\"kind\":{{{}}},\"maturity\":{{{}}},\"branches\":{},\"map_paths\":{}}}",
                jstr(lib.name()),
                recs.len(),
                promoted,
                dump(&kinds),
                dump(&mats),
                branches,
                paths
            ));
            continue;
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
    if a.has("json") {
        println!("{{\"libraries\":[{}]}}", json_rows.join(","));
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
\"reason\":{},\"body\":{},\"created\":{},{},\"attach_count\":{},\"attach_bytes\":{}}}",
                    jstr(h.lib.name()),
                    jstr(&h.path),
                    jstr(&h.title),
                    jstr(h.tier.label()),
                    h.score,
                    jstr(&h.reason),
                    jstr(&h.record.as_ref().map(|r| r.body.clone()).unwrap_or_default()),
                    h.record.as_ref().map(|r| r.created()).unwrap_or(0),
                    meta_json(h.updated, Some(h.bytes as u64), Some(h.lines)),
                    h.attach_count,
                    h.attach_bytes
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
    // 命中行里的路径也换短号：一次召回 5 条就是 5 个 48 字符路径。
    // 完整编号用 `abook num --of <短号>` 反查。
    let nums = crate::num::Numbers::load_or_build(&root);
    for h in &res.hits {
        // Score order stays; the date rides along so a hit can be placed in
        // time. Without it a reader cannot tell "this is how it works now" from
        // "this was how it worked a year ago".
        // 前缀统一走 `row_meta`，附件列按 `list` 的规矩：没有就不占位。
        let meta = crate::util::row_meta(h.updated, Some(h.bytes as u64), Some(h.lines));
        let att = if h.attach_count == 0 {
            String::new()
        } else {
            format!("  {}件{}", h.attach_count, human_bytes(h.attach_bytes))
        };
        println!(
            "{meta}  [{}{}] {} {}  ({} / {})",
            h.lib.name(),
            att,
            h.title,
            {
                let id = h.path.rsplit('/').next().unwrap_or(&h.path).trim_end_matches(".md");
                nums.num_of(id).map(|n| n.to_string()).unwrap_or_else(|| h.path.clone())
            },
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

/// Advise the caller which version it just read.
///
/// Suppressed under `--json` (where `version` is a real field) and under
/// `--quiet`, so machine consumers and scripts that pipe the body are unaffected.
fn print_version_footer(ver: &str, a: &Args) {
    if a.has("json") || a.has("quiet") {
        return;
    }
    // The fingerprint appears exactly once: a footer that repeats it (once as
    // "[version X]" and once inside a suggested command) makes `grep -o` and any
    // other simple extractor return two values, and a caller that takes them all
    // ends up passing a two-line fingerprint -- which then reproduces the version
    // twice inside the mismatch message.
    eprintln!("[version {ver}] 写回时带上 --if-version 可防止覆盖他人改动");
}

pub fn cmd_show(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录选择器（路径 / 文件名 / ID）")?;
    let (path, lib) = resolve_record(&root, sel)?;
    let rec = Record::parse(&path, lib)?;
    // `cat` and `show` shared one handler, so `cat` printed the whole record
    // while the help screen promised "只输出正文". A caller piping `cat` into
    // something that expects prose got headers and a body glued together.
    // Both names still route here; only the output differs.
    if a.cmd == "cat" {
        let ver = file_version(&path);
        if a.has("json") {
            println!(
                "{{\"path\":{},\"lib\":{},\"body\":{},\"version\":{},\"updated\":{}}}",
                jstr(&rel(&root, &path)),
                jstr(lib.name()),
                jstr(&rec.body),
                jstr(&ver),
                rec.updated()
            );
        } else {
            print!("{}", rec.body);
            if !rec.body.ends_with('\n') {
                println!();
            }
            // The version goes last so it never contaminates piped body text, and
            // it is printed at all so a caller can hand it back on write. Without
            // it, "read then edit" has no way to notice that something changed in
            // between -- the write simply wins.
            print_version_footer(&ver, a);
        }
        return Ok(0);
    }
    if a.has("json") {
        println!(
            "{{\"path\":{},\"lib\":{},\"fields\":{{{}}},\"body\":{},\"version\":{},\"deviations\":{},\"created\":{},\"updated\":{},\"when\":{}}}",
            jstr(&rel(&root, &path)),
            jstr(lib.name()),
            rec.fields
                .iter()
                .map(|(k, v)| format!("{}:{}", jstr(k), jstr(v)))
                .collect::<Vec<_>>()
                .join(","),
            jstr(&rec.body),
            jstr(&file_version(&path)),
            rec.devs.len(),
            rec.created(),
            rec.updated(),
            jstr(&crate::util::short_when(rec.updated()))
        );
        return Ok(0);
    }
    // `--field` is the natural thing for a caller to reach for when it only
    // wants one value (a script asking for ID, or an AI checking STATUS).
    // It used to be silently ignored, which quietly returned the whole record
    // and made any downstream parse wrong.
    if let Some(want) = a.get("field") {
        if !want.trim().is_empty() {
            let names: Vec<&str> = want.split(',').map(|x| x.trim()).filter(|x| !x.is_empty()).collect();
            let mut out = 0usize;
            for name in &names {
                if name.eq_ignore_ascii_case("body") || name.eq_ignore_ascii_case("__body__") {
                    print!("{}", rec.body);
                    if !rec.body.ends_with('\n') {
                        println!();
                    }
                    out += 1;
                    continue;
                }
                // exact key first, then case-insensitive, so callers who write
                // `--field title` still get TITLE instead of an empty line.
                let hit = rec
                    .fields
                    .iter()
                    .find(|(k, _)| k == name)
                    .or_else(|| rec.fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)));
                match hit {
                    Some((_, v)) => {
                        println!("{v}");
                        out += 1;
                    }
                    None => {
                        eprintln!("警告: 记录没有字段 {name}");
                    }
                }
            }
            if out == 0 {
                return Ok(4);
            }
            return Ok(0);
        }
    }

    println!("{}  [{}]", rel(&root, &path), lib.name());
    // Rendered here rather than read out of the header, so records written
    // before `UPDATED` existed still show a date instead of nothing. The
    // fallback chain lives in `Record::updated`.
    println!(
        "  时间  {}（改动）  {}（建立）",
        crate::util::short_when(rec.updated()),
        crate::util::short_when(rec.created())
    );
    for (k, v) in &rec.fields {
        let vv = if v.chars().count() > 200 && !a.has("verbose") {
            truncate_chars(v, 200)
        } else {
            v.clone()
        };
        println!("  {k}={vv}");
    }
    println!();
    // The body of a long record was printed whole no matter what `--page`
    // said, because the copy that actually honoured paging (`cmd_view::
    // cmd_show`) was never wired into the dispatch table. The two had drifted
    // apart; this is the surviving one, now carrying the paging.
    // Always run the budget check, not only when `--page` was passed.
    //
    // Requiring a flag meant the default path printed the whole body, so a
    // 120 KB record came back in full unless the caller already knew it was
    // long -- and a caller cannot know that before reading it. The budget is
    // the default; the flags only override it.
    let (body, footer) = crate::paging::text_page(&rec.body, a);
    println!("{body}");
    if !footer.is_empty() {
        println!("{footer}");
    }
    if let Some(p) = &rec.promotion {
        println!();
        println!("[PROMOTION]");
        println!("{}", p.trim_end());
    }
    // Printed after everything else so it is never mistaken for record content,
    // and always printed so a caller can carry it into the write. See
    // `util::file_version` for why the caller must hold it across the gap.
    print_version_footer(&file_version(&path), a);
    // Deviations are the tolerance ledger: which rule fired, on what text.
    // Without `--verbose`/`--deviations` they stay hidden so ordinary output
    // is unchanged.
    if !rec.devs.is_empty() && (a.has("verbose") || a.has("deviations")) {
        println!();
        println!("[解析纠偏 {} 处]", rec.devs.len());
        for d in &rec.devs {
            println!("  {}", d.show());
        }
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
    // Same index `tree` builds: one pass over the library, then per-branch sums.
    // `atlas` and `tree` answering the same question with different numbers
    // would be worse than either answering it alone.
    #[derive(Clone, Copy, Default)]
    struct Meta {
        updated: i64,
        bytes: usize,
        lines: usize,
        attach_count: usize,
        attach_bytes: u64,
    }
    let mut index: std::collections::HashMap<String, Meta> = std::collections::HashMap::new();
    for l in [Lib::Canonical, Lib::Candidate] {
        for r in crate::rec::load_lib(&root, l) {
            let a = crate::attach::scan(&r.path);
            let m = Meta {
                updated: r.updated(),
                bytes: r.raw.len(),
                lines: count_lines(&r.raw),
                attach_count: a.count(),
                attach_bytes: a.bytes,
            };
            index.insert(format!("{}/{}", l.dir(), r.file_name()), m);
            index.insert(r.file_name(), m);
        }
    }
    let agg = |b: &crate::atlas::Branch| -> (usize, i64, Option<u64>, Option<usize>, usize, u64) {
        let mut seen: Vec<String> = Vec::new();
        let (mut latest, mut bytes, mut lines) = (0i64, 0u64, 0usize);
        let mut measured = 0usize;
        let (mut ac, mut ab) = (0usize, 0u64);
        for p in b.paths() {
            let bare = p.rsplit('/').next().unwrap_or(&p).to_string();
            if seen.contains(&bare) {
                continue;
            }
            seen.push(bare.clone());
            let m = match index.get(&p).or_else(|| index.get(&bare)) {
                Some(m) => *m,
                // PATH 指向的文件不在盘上（记录改过名）。它仍然是这条分支声明
                // 的成员，但量不出体积——不能按 0 计入，否则"每条成员都丢了"和
                // "这条分支本来就是空的"会显示成同一个样子。
                None => continue,
            };
            measured += 1;
            if m.updated > latest {
                latest = m.updated;
            }
            bytes += m.bytes as u64;
            lines += m.lines;
            ac += m.attach_count;
            ab += m.attach_bytes;
        }
        let unmeasured = measured == 0 && !seen.is_empty();
        (
            seen.len(),
            latest,
            if unmeasured { None } else { Some(bytes) },
            if unmeasured { None } else { Some(lines) },
            ac,
            ab,
        )
    };
    if a.has("json") {
        // Dedicated branch: the human path interleaves headers with a --card
        // drill-down, and folding JSON into it would emit prose on some inputs.
        let mut out: Vec<String> = Vec::new();
        for lib in libs {
            let p = root.join(lib.atlas());
            if !p.exists() {
                out.push(format!(
                    "{{\"lib\":{},\"exists\":false}}",
                    jstr(lib.name())
                ));
                continue;
            }
            let at = Atlas::load(&p)?;
            let only = a.get("card");
            let cards = at
                .branches
                .iter()
                .filter(|b| match only {
                    Some(w) => b.name() == w || b.heading == w,
                    None => true,
                })
                .map(|b| {
                    let fields = b
                        .fields
                        .iter()
                        .map(|(k, v)| format!("{}:{}", jstr(k), jstr(v)))
                        .collect::<Vec<_>>()
                        .join(",");
                    // 元信息与文本同一份：文本那份早就带着时间/体积/行数，
                    // JSON 只有 declared/parsed/paths。同一张卡两侧依据不同，
                    // 机器调用方只能盲取。
                    let (n, ts, by, ln, ac, ab) = agg(b);
                    format!(
                        "{{\"branch\":{},\"heading\":{},\"declared_count\":{},\"parsed_count\":{},\"count\":{},\"paths\":{},{},\"attach_count\":{},\"attach_bytes\":{},\"fields\":{{{fields}}}}}",
                        jstr(&b.name()),
                        jstr(&b.heading),
                        b.get("COUNT").parse::<i64>().unwrap_or(-1),
                        b.count(),
                        n,
                        b.paths().len(),
                        meta_json(ts, by, ln),
                        ac,
                        ab
                    )
                })
                .collect::<Vec<_>>();
            let all_cards = cards.len();
            let (cards, pg) = crate::paging::page_json(a, cards);
            out.push(format!(
                "{{\"lib\":{},\"exists\":true,\"path\":{},\"branches\":{},\"total_count\":{},\"covered_through\":{},{pg},\"card_total\":{all_cards},\"cards\":[{}]}}",
                jstr(lib.name()),
                jstr(lib.atlas()),
                at.branches.len(),
                at.total_count(),
                jstr(&at.get("COVERED_THROUGH")),
                cards.join(",")
            ));
        }
        println!("[{}]", out.join(","));
        return Ok(0);
    }
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
        let rows: Vec<String> = at
            .branches
            .iter()
            .map(|b| {
                if a.has("verbose") {
                    format!(
                        "   {:<24} 声明={:<4} 实析={:<4} PATH原文={}",
                        b.name(),
                        b.get("COUNT"),
                        b.count(),
                        b.paths().len()
                    )
                } else {
                    let (n, ts, by, ln, ac, ab) = agg(b);
                    // 列序跟 `row_meta` 一致：时间 / 体积 / 行数 在最前，条目数
                    // 这类命令特有列接在后面。这里原来是「条目数 体积 行数 附件
                    // 时间」，正是"同一份数据四种排列"里的反例。
                    format!(
                        "{}  {n:>4}条  {:<24}  {:<11} REV={}  {}",
                        crate::util::row_meta(ts, by, ln),
                        b.name(),
                        if ac == 0 { String::new() } else { format!("{}件{}", ac, human_bytes(ab)) },
                        b.get("REVISION"),
                        truncate_chars(&b.get("KEYS"), 60)
                    )
                }
            })
            .collect();
        let (shown, footer) = crate::paging::page_rows(a, rows);
        for r in &shown {
            println!("{r}");
        }
        if !footer.is_empty() {
            println!("{footer}");
        }
    }
    Ok(0)
}

/// `identity` — the credential identity index in Global.md.
///
/// The spec makes this section a per-turn read: it says what credentials exist,
/// what each may be used for, and at what disclosure level -- everything needed
/// to decide whether to fetch one, with no values in it. `BookFile` could parse
/// it from the start and nothing ever called the parser, so the one section
/// designed to be read every turn had no way to be read at all. Callers fell
/// back to `vault ledger`, which lives inside the encrypted vault and therefore
/// requires decrypting the whole box to learn only the names.
///
/// Columns are positional in the source and the last one can be long; the table
/// form here keeps the identifier and the purpose together at the front because
/// that is what a caller matches on.
pub fn cmd_identity(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let p = root.join("Global.md");
    if !p.exists() {
        return notfound("Global.md 不存在");
    }
    let bf = BookFile::load(&p)?;
    let rows = bf.credential_rows();
    if rows.is_empty() {
        // Not an error: a library may legitimately have no credentials yet.
        // Saying so beats an empty table that looks like a parse failure.
        if a.has("json") {
            println!("{{\"count\":0,\"total\":0,\"identities\":[]}}");
        } else {
            println!("Global.md 的 Credential Identity Index 为空。");
        }
        return Ok(0);
    }
    let q = a.pos.first().cloned().unwrap_or_default();
    let hit: Vec<(usize, &Vec<String>)> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| q.is_empty() || r.iter().any(|c| ci_contains(c, &q)))
        .collect();
    if hit.is_empty() {
        return notfound(&format!("身份索引里没有匹配 {q} 的条目"));
    }
    if a.has("json") {
        let items: Vec<String> = hit
            .iter()
            .map(|(_, r)| {
                let get = |i: usize| jstr(r.get(i).map(|s| s.as_str()).unwrap_or(""));
                format!(
                    "{{\"id\":{},\"purpose\":{},\"owner\":{},\"allowed\":{},\"when\":{},\"disclosure\":{}}}",
                    get(0),
                    get(1),
                    get(2),
                    get(3),
                    get(4),
                    get(5)
                )
            })
            .collect();
        let total = items.len();
        let (shown, pg) = crate::paging::page_json(a, items);
        println!(
            "{{\"count\":{},\"total\":{total},{pg},\"identities\":[{}]}}",
            shown.len(),
            shown.join(",")
        );
        return Ok(0);
    }
    println!("Global.md 凭据身份索引：{} 条（不含任何凭据值）", rows.len());
    let lines: Vec<String> = hit
        .iter()
        .map(|(_, r)| {
            let id = r.first().map(|s| s.as_str()).unwrap_or("");
            let purpose = r.get(1).map(|s| s.as_str()).unwrap_or("");
            let when = r.get(4).map(|s| s.as_str()).unwrap_or("");
            format!("  {id}\n    用途: {purpose}\n    取用: {when}")
        })
        .collect();
    let (shown, footer) = crate::paging::page_rows(a, lines);
    for l in &shown {
        println!("{l}");
    }
    if !footer.is_empty() {
        println!("{footer}");
    }
    println!("取单条值: abook vault get <标识>");
    Ok(0)
}

/// `Index.md` 里一条路由是"单条记录"还是"分支卡"。
///
/// 判据只此一份。此前 JSON 侧用 `path.contains("/records/")`、文本侧用
/// `contains("/records/") || ends_with(".md")`——同一条路由可能在文本里进"记录"
/// 段、在 JSON 里被标成 `branch`。两个说法同时存在时，总有一个在骗人。
///
/// 合并后的判据取两侧的**并集**，谁的条件都不丢：`/records/` 里的、以 `.md`
/// 结尾的、以及能在记录索引里查到的（给不带扩展名的裸名路由兜底）。只看路径形状
/// 会把一条指向已删文件的记录路由错判成分支，只看索引又会把同样一条错判成"没有
/// 这条记录"。两条合起来才是"这条路由声明的是什么"。
///
/// `known` 由调用方给（文本与 JSON 两侧都已经建过记录索引）。
fn index_kind(path: &str, known: bool) -> &'static str {
    if path.contains("/records/") || path.ends_with(".md") || known {
        "record"
    } else {
        "branch"
    }
}

pub fn cmd_index(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    // `--plane D5` / `--plane R3` 取回该条全文。
    //
    // 平面只给摘要，必须留一条回到原文的路，否则"看得见摘要却拿不到正文"
    // 比不显示更糟——读者知道有这条规矩，却没法照着做。
    if let Some(sel) = a.get("plane") {
        // `--plane all`：把两类都列全，不受上限截断。
        //
        // 上限是为了"预加载别太大"，不是"不许看全"。提示里说了可以看全
        // 就必须真能看全，否则那句话本身就是假的。
        if sel == "all" || sel == "none" {
            let (mut dirs, mut rules, _) = plane_view_all(&root);
            let cap = if sel == "none" { 0 } else { usize::MAX };
            dirs.truncate(cap);
            rules.truncate(cap);
            if a.has("json") {
                let dj: Vec<String> = dirs.iter().map(|(n, s, _)| format!("{{\"id\":{},\"summary\":{}}}", jstr(n), jstr(s))).collect();
                let rj: Vec<String> = rules.iter().map(|(n, s, _)| format!("{{\"id\":{},\"summary\":{}}}", jstr(n), jstr(s))).collect();
                println!("{{\"directive\":[{}],\"rule\":[{}]}}", dj.join(","), rj.join(","));
            } else {
                if !dirs.is_empty() {
                    println!("【用户交代】共 {} 条", dirs.len());
                    for (i, (n, s, _)) in dirs.iter().enumerate() {
                        println!("  D{}｜{s}｜{n}", i + 1);
                    }
                }
                if !rules.is_empty() {
                    println!("【通用强约束】共 {} 条", rules.len());
                    for (i, (n, s, _)) in rules.iter().enumerate() {
                        println!("  R{}｜{s}｜{n}", i + 1);
                    }
                }
            }
            return Ok(0);
        }
        {
            let (dirs, rules, _) = plane_view(&root, a);
            let idx: Option<usize> = sel
                .trim_start_matches(['D', 'd', 'R', 'r'])
                .parse()
                .ok()
                .map(|n: usize| n.saturating_sub(1));
            let row = match sel.chars().next() {
                Some('D') | Some('d') => idx.and_then(|i| dirs.get(i)),
                Some('R') | Some('r') => idx.and_then(|i| rules.get(i)),
                _ => None,
            };
            let (name, _, _) = match row {
                Some(r) => r,
                None => return notfound(&format!("没有 {sel} 这一条")),
            };
            let p = root.join("canonical/records").join(name);
            let body = std::fs::read_to_string(&p)?;
            // --json 必须给可解析的 JSON：正文塞进字符串字段。
            // 直接 print! 会让 `index --plane D5 --json` 吐出裸 Markdown，
            // 调用方解析失败——自检正是抓这个。
            if a.has("json") {
                println!("{{\"plane\":{},\"path\":{},\"body\":{}}}",
                    jstr(sel), jstr(&name), jstr(&body));
            } else {
                print!("{body}");
            }
            return Ok(0);
        }
    }
    let p = root.join("Index.md");
    if !p.exists() {
        return notfound("Index.md 不存在");
    }
    let bf = BookFile::load(&p)?;
    let routes = bf.routes();
    let q = a.pos.first().cloned().unwrap_or_default();
    // 只 stat + 数行，不走 `load_lib`。
    //
    // 这里的差别不是微调：`load_lib` 会把每条记录解析成结构体并克隆几份，965 条
    // 要 80ms；而这条命令是"每轮预载先看一眼"的视图，80ms 让它在预载路径上成为
    // 最贵的一步。实测原始读+数行（965 个文件、5.4MB）只要 6ms，纯 stat 1ms——
    // 那 80ms 全在解析，而索引只需要字节数与行数，不需要任何字段语义。
    // 元信息三件套。缺的维度是 `None`，不是 0：JSON 里写 0 会被读成"这条
    // 记录是空的"，而真相是"这一维度量不出来"。
    let mut rec_index: std::collections::HashMap<String, (Option<u64>, Option<usize>, i64)> =
        std::collections::HashMap::new();
    // 已晋升的候选：文件名（bare）→ 目标正式 ID。
    //
    // 为什么在这里顺手判：索引要为每条记录算出字节数与行数，本来就 `fs::read`
    // 过整个文件了，判断"是否已晋升"只需要在已读到的字节里找两个标记，零额外
    // I/O。而它决定下面那个「未晋升候选」段到底列什么——不判它，那一段就是把
    // 全部候选（含已晋升的溯源副本）当成待办列出来，读者看到几百条会以为
    // 积压严重，实际待办可能是 0。
    let mut promoted: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        // lib.dir() 已经是 "canonical/records"，再拼一次 records 会让整个索引
        // 建在空目录上——所有元信息退化成 "-"，而且没有报错。
        let dir = root.join(lib.dir());
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let name = match p.file_name() {
                Some(n) => n.to_string_lossy().to_string(),
                None => continue,
            };
            if !name.ends_with(".md") {
                continue;
            }
            let ts = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            // 读不出来不跳过：索引漏掉一条会让读者以为这条记录不存在，而它其实
            // 存在、只是读不了。但也**不能记成 0 字节 0 行**——那是对数据的错误
            // 陈述，读者会判断成"这条是空的"而不是"读不了"。给 `None`，文本侧打
            // `-`、JSON 侧写 null。
            let (bytes, lines) = match std::fs::read(&p) {
                Ok(b) => {
                    // 同一批字节里顺手取晋升标记，不额外开文件。
                    // 两个标记覆盖两代写法：末尾 `[PROMOTION]` 块与头部
                    // `PROMOTED_TO=` 字段。`找不到就说明没晋升过`是这个字段的
                    // 全部语义，所以这里只做"有没有"的判断，不去解析值。
                    //
                    // **必须锚定行首，不能全文找子串。**
                    //
                    // 原来写的是 `b.windows(12).any(|w| w == b"PROMOTED_TO=")`，
                    // 那是全文子串搜索：只要**正文里提到**这个字段名就命中。
                    // 实测踩到了——一条候选的正文写着
                    // `grep "^PROMOTED_TO=" f  # 对：行首锚定`
                    // （它恰好在记录"查字段必须锚定行首"这条教训），于是它自己
                    // 被扫成已晋升，从索引的待办列表里消失，还把那句"另有 N 条
                    // 已晋升"的数抬高了 1。
                    //
                    // 真实危害不是数字错一位，而是**一条真候选变得不可见**：
                    // 索引是预载入口，看不见就等于不待处理，它会一直躺在库里
                    // 没人读。教训写在正文里、被同一个 bug 吃掉，是这类缺陷最
                    // 典型的形态。
                    if lib == Lib::Candidate {
                        let at_line_start = |pat: &[u8]| -> bool {
                            b.split(|&c| c == b'\n').any(|line| line.starts_with(pat))
                        };
                        // 容忍 CRLF：行首可能带 `\r` 的只有行尾，不影响 starts_with。
                        let hit = at_line_start(b"PROMOTED_TO=")
                            || at_line_start(b"[PROMOTION]");
                        if hit {
                            promoted.insert(name.clone(), String::new());
                        }
                    }
                    (Some(b.len() as u64), Some(count_lines_bytes(&b)))
                }
                Err(_) => (None, None),
            };
            let full = format!("{}/{}", lib.dir(), name);
            rec_index.insert(full, (bytes, lines, ts));
            rec_index.insert(name, (bytes, lines, ts));
        }
    }
    // 分支聚合:与 `tree` 同一口径,按 basename 去重后求和。
    // `(条目数, 体积, 行数, 最近活跃)`；体积/行数是 `None` = 成员一条都没量到。
    let mut branch_agg: std::collections::HashMap<String, (usize, Option<u64>, Option<usize>, i64)> =
        std::collections::HashMap::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        let ap = root.join(lib.atlas());
        if !ap.exists() {
            continue;
        }
        if let Ok(at) = crate::atlas::Atlas::load(&ap) {
            for b in &at.branches {
                let mut seen: Vec<String> = Vec::new();
                let (mut bytes, mut lines, mut latest) = (0u64, 0usize, 0i64);
                let mut measured = 0usize;
                for p in b.paths() {
                    let bare = p.rsplit('/').next().unwrap_or(&p).to_string();
                    if seen.contains(&bare) {
                        continue;
                    }
                    seen.push(bare.clone());
                    let m = rec_index.get(&p).or_else(|| rec_index.get(&bare));
                    if let Some((by, ln, ts)) = m {
                        if let (Some(by), Some(ln)) = (by, ln) {
                            measured += 1;
                            bytes += by;
                            lines += ln;
                        }
                        if *ts > latest {
                            latest = *ts;
                        }
                    }
                }
                // 一条成员都量不到时给 `None`，不要拿 0 冒充"这条分支是空的"。
                let unmeasured = measured == 0 && !seen.is_empty();
                branch_agg.insert(
                    b.name(),
                    (
                        seen.len(),
                        if unmeasured { None } else { Some(bytes) },
                        if unmeasured { None } else { Some(lines) },
                        latest,
                    ),
                );
            }
        }
    }

    if a.has("json") {
        // Routes carry the full-width ｜ separator because a half-width one
        // appears inside route values (AB2V1|nonce|tag); the split already
        // honours that, so JSON just re-emits the three real fields.
        // `--limit` used to be applied *before* paging and defaulted to 40, so
        // `index --json --page 2 --per-page 10` sliced a 40-row set and came
        // back empty forever. `page_json` applies the cap first and pages the
        // capped set, the only order that keeps both flags meaningful. The 40
        // default is gone as well: an unpaged machine-readable inventory that
        // silently stops at 40 rows is the same lie as ignoring the flag.
        let items: Vec<String> = routes
            .iter()
            .filter(|(path, summary, when)| {
                q.is_empty()
                    || ci_contains(path, &q)
                    || ci_contains(summary, &q)
                    || ci_contains(when, &q)
            })
            .map(|(path, summary, when)| {
                // `kind` 区分分支与记录级路由。
                //
                // 文本输出已经把两类分段打印了,但机器调用方拿到的是一层扁平
                // 数组:读者看得见"这条是分支、那条是单条记录",JSON 调用方看不出
                // 来,只能靠 path 里有没有 "/records/" 去猜——那是字符串形状的
                // 巧合,不是契约。`status` 再分出 candidate,让"候选只可参考"这条
                // 语义在机器侧同样成立。
                //
                // 判据只有一份（`index_kind`）:这里原来是 `path.contains(
                // "/records/")`，文本侧是 `|| path.ends_with(".md")`，于是同一条
                // 路由可能在文本里进"记录"段、在 JSON 里被标成 branch。两种说法
                // 同时存在时，总有一个在骗人。
                let bare = path.rsplit('/').next().unwrap_or(path).to_string();
                let known = rec_index.contains_key(path) || rec_index.contains_key(&bare);
                let kind = index_kind(path, known);
                // 机器侧同样要能区分"候选待办"与"候选已晋升的溯源副本"。
                // 只给 candidate/canonical 两级，JSON 调用方必须自己再开一次
                // 文件才知道这条要不要处理——而这条记录刚被读过一遍。
                let status = if path.starts_with("candidate/") {
                    if promoted.contains_key(&bare) {
                        "promoted"
                    } else {
                        "candidate"
                    }
                } else if kind == "record" {
                    "canonical"
                } else {
                    // 分支卡覆盖两个库（Index.md 的"正式 / 候选"列即计数），
                    // 所以它不属于任何一侧；写 branch 会与 kind 重复且无信息。
                    "both"
                };
                // 元信息与文本输出同一份数据:体积/行数/最近活跃/条目数。
                // 文本读者看得见的东西,机器调用方也必须看得见——否则"要不要读"
                // 这个判断在两侧的可用依据不同,机器侧只能盲取。
                let (by, ln, ts, cnt) = if kind == "record" {
                    match rec_index.get(path).or_else(|| rec_index.get(&bare)) {
                        Some((b, l, t)) => (*b, *l, *t, Some(1usize)),
                        // 路由指向的文件不在盘上：量不出来，不是 0 字节。
                        None => (None, None, 0, None),
                    }
                } else {
                    match branch_agg.get(path) {
                        Some((n, b, l, t)) => (*b, *l, *t, Some(*n)),
                        None => (None, None, 0, None),
                    }
                };
                format!(
                    "{{\"path\":{},\"kind\":{},\"status\":{},\"summary\":{},\"read_when\":{},\"count\":{},{}}}",
                    jstr(path),
                    jstr(kind),
                    jstr(status),
                    jstr(summary),
                    jstr(when),
                    jopt(cnt),
                    meta_json(ts, by, ln)
                )
            })
            .collect();
        // 凭据与路由并列,而不是要求调用方"顺便再跑一次 vault ls"。
        // 索引的用途是让调用方一眼看到"库里有什么、因此可以怎么approach";
        // 只列知识分支不列凭据,等于把"能用什么去访问外部系统"这个维度藏起来,
        // 调用方会退化成只查知识、想不起还有 token 可用。
        let creds = credential_index(&root);
        let cjson: Vec<String> = creds
            .iter()
            .filter(|(id, purpose)| q.is_empty() || ci_contains(id, &q) || ci_contains(purpose, &q))
            .map(|(id, purpose)| {
                format!("{{\"ident\":{},\"purpose\":{},\"fetch\":{}}}", jstr(id), jstr(purpose), jstr(&format!("abook vault get {id}")))
            })
            .collect();
        let matched_n = items.len();
        let (shown, pg) = crate::paging::page_json(a, items);
        // Count real items; counting occurrences of a substring in the emitted
        // text would break the moment a path itself contains that substring.
        let shown_n = shown.len();
        let body = shown.join(",");
        println!(
            "{{\"routes\":{},\"matched\":{matched_n},\"shown\":{shown_n},{pg},\"credentials\":[{}],\"items\":[{body}]}}",
            routes.len(),
            cjson.join(",")
        );
        return Ok(0);
    }
    // ---- 预加载平面：三类分组，前两类必须在这里全见 ----
    //
    // 这一段的用途不是"检索"，是"开局印象"。模型不查也能看见用户交代过什么、
    // 有哪些硬约束，于是不会在"该不该做某件事"上失手。
    //
    // 体积靠两道闸兜住，而不是靠"少列几条"：
    //   ① 每条只给一行摘要（SUMMARY 首句截断），不是正文；
    //   ② 每类有硬上限，超出时按最近活跃排序截断并如实报"还有 N 条"。
    //      报数而不静默丢弃——静默截断会让"平面看起来完整"变成谎言。
    let (plane_dirs, plane_rules, plane_dropped) = plane_view(&root, a);
    // 先把每条路由的元信息算出来。分支走 Atlas 聚合（条目数/体积/行数/最近活跃），
    // 记录级路由直接指向单个文件。
    //
    // 这两类在 Index.md 里住在同一张表,但它们的"体积"含义不同:分支是成员求和,
    // 记录是一条文件。混在一起打印会让读者以为 72 和 8.3KB 是同一量纲。
    // 记录与分支走同一个分类判据，理由见 JSON 分支里 `index_kind` 的注释。
    //
    // 记录级路由再分两堆：正式的那些是**结论**，候选里还没晋升的那些是**线索**。
    // 原来只有一堆，段标题写「未晋升候选」却把正式记录也印在里面，同时把已晋升
    // 的候选副本（只是溯源对象）也列成待办——标题和内容两头都不对。分开之后
    // 各段只装自己那一类，数字也才有意义。
    let (mut br_rows, mut canon_rows, mut cand_rows): (Vec<String>, Vec<String>, Vec<String>) =
        (Vec::new(), Vec::new(), Vec::new());
    // 解析不到单条文件的候选路由（地图文件、`A..B` 区间指针），单独收。
    // 它们既不是待办也不是结论，混进任何一段都会让那一段的数字失真。
    let mut pointer_rows: Vec<String> = Vec::new();
    // 命中的已晋升候选数：不打印内容，只报个数，并说明去哪看。
    // 完全不提也不行——读者会以为索引漏了这些记录。
    let mut promoted_hidden = 0usize;
    for (path, summary, when) in &routes {
        if !q.is_empty() && !ci_contains(path, &q) && !ci_contains(summary, &q) && !ci_contains(when, &q) {
            continue;
        }
        let bare = path.rsplit('/').next().unwrap_or(path).to_string();
        let known = rec_index.contains_key(path) || rec_index.contains_key(&bare);
        if index_kind(path, known) == "record" {
            let meta = rec_index.get(path).or_else(|| rec_index.get(&bare));
            let (by, ln, ts) = match meta {
                Some((b, l, t)) => (*b, *l, *t),
                None => (None, None, 0),
            };
            let is_cand = path.starts_with("candidate/");
            // 「未晋升候选」这个数字必须精确到"真有几条要处理"，否则它比不报还坏：
            // 实测库里 299 条候选，其中 296 条是已晋升的溯源副本、2 条是地图与区间
            // 指针、1 条才是真待办。全算进去会读成 299 条积压。
            //
            // 判据只能落在**这条路由能不能解析成一条真记录**上：解析不到的是指针
            // （`candidate/Atlas.md`、`AB2-N-073.md..AB2-N-079.md` 这类），它对
            // 应若干条记录或一个文件，本身没有晋升状态可言。
            if is_cand {
                if !known {
                    pointer_rows.push(format!(
                        "{}  [指针] {}｜{}",
                        crate::util::row_meta(ts, by, ln),
                        path,
                        summary
                    ));
                    continue;
                }
                if promoted.contains_key(&bare) {
                    promoted_hidden += 1;
                    continue;
                }
            }
            let row = format!(
                "{}  [{}] {}｜{}",
                crate::util::row_meta(ts, by, ln),
                if is_cand { "候选" } else { "正式" },
                path,
                summary
            );
            if is_cand {
                cand_rows.push(row);
            } else {
                canon_rows.push(row);
            }
        } else {
            let agg = branch_agg.get(path);
            let (n, by, ln, ts) = match agg {
                Some((n, b, l, t)) => (Some(*n), *b, *l, *t),
                None => (None, None, None, 0),
            };
            let nshow = match n {
                Some(x) => format!("{x:>4}条"),
                None => "    -".to_string(),
            };
            br_rows.push(format!(
                "{}  {nshow}  {path}｜{when}",
                crate::util::row_meta(ts, by, ln)
            ));
        }
    }

    // 三个数字都取自**实际要打印的行**，不取中间集合的大小。
    //
    // 原来写的是 `routes.len()` 和 `branch_agg.len()`：前者是过滤前的总数，带
    // `--q` 时就不对；后者是 Atlas 聚合表的条目数，那份表里可能有一个分支根本
    // 没有对应的 Index.md 路由，于是标题出现 "路由 26 条（分支 20 / 记录 7）"
    // 这种自相矛盾的算术（20+7=27）。实际印出来的是 19 行分支 + 7 行记录。
    //
    // 抬头数字与实际行数不符是读者最先会去数的地方，数一次对不上就再也不敢信
    // 这张表。
    let n_branch = br_rows.len();
    let n_rec = canon_rows.len() + cand_rows.len() + pointer_rows.len();
    println!("Index.md  路由 {} 条（分支 {} / 记录 {}）", n_branch + n_rec, n_branch, n_rec);
    // 已晋升的候选不在上面三个数字里，但必须交代一句：不然读者会以为索引漏了
    // 这些记录。它们仍然可读（`abook show <路径>`），只是默认视图里属于历史。
    if promoted_hidden > 0 {
        println!(
            "           另有 {} 条候选已晋升，只作溯源保留，不列为待办（`list --include-promoted` 可看）",
            promoted_hidden
        );
    }
    if !br_rows.is_empty() {
        println!();
        // 平面先于分支打印：读者第一眼该看到"非做不可的事"，不是"去哪儿翻"。
        // 顺序即优先级，输出顺序会被当成重要性的暗示。
        if !plane_dirs.is_empty() {
            println!("【用户交代】你让我记住的 —— 开局全见，不用查");
            for (i, (name, summary, _ts)) in plane_dirs.iter().enumerate() {
                // 不带完整文件名：它是 22 字符的固定长度噪声，43 条就是近 1KB，
                // 而读者要的是"有哪些规矩"，不是文件名。要看全文用序号取。
                let _ = name;
                println!("  D{}｜{summary}", i + 1);
            }
            if plane_dropped.0 > 0 {
                println!("   ……另有 {} 条较早交代未列（--plane all 可看全）", plane_dropped.0);
            }
        }
        if !plane_rules.is_empty() {
            println!("【通用强约束】约束 / 决策 / 纠正 / 教训 —— 动手前先看");
            for (i, (name, summary, _ts)) in plane_rules.iter().enumerate() {
                let _ = name;
                println!("  R{}｜{summary}", i + 1);
            }
            if plane_dropped.1 > 0 {
                println!("   ……另有 {} 条较早约束未列（--plane all 可看全）", plane_dropped.1);
            }
        }
        println!("【知识分支】时间 / 体积 / 行数 / 条目数 —— 聚合值，据此判断该不该进去翻");
    }
    // `--per-page 0` 表示"不分页"，`--limit` 是总数上限——两者不是一回事，
    // 不能被同一个默认值 40 吃掉。
    // 默认只列 12 条：平面段（用户交代 / 通用强约束）已经把最重要的铺开了，
    // 这里再列 40 条会把预加载撑到 15KB 以上，而多出来的那些多数用不上。
    // 需要看全的调用方显式给 --limit 0。
    let lim = match a.num("limit") {
        Some(0) => usize::MAX,
        Some(n) => n as usize,
        None => 12,
    };
    let cut_b = br_rows.len().min(lim);
    // `--limit` 先截再分页,两条 flag 各自独立,与其余 list 命令一致。
    let (shown_rows, footer) = crate::paging::page_rows(a, br_rows[..cut_b].to_vec());
    for r in &shown_rows {
        println!("{r}");
    }
    if cut_b < br_rows.len() {
        println!("  …（另有 {} 条，用 --limit 扩大）", br_rows.len() - cut_b);
    }
    if !footer.is_empty() {
        println!("{footer}");
    }

    // 正式记录段：这些是已闭合、经用户批准的结论。
    //
    // 候选段先分名额，正式段吃剩下的——顺序不能反。候选是**待办**，正式记录
    // 是**已有资产**；`--limit` 小的时候把额度全给"已经有 205 条的东西"，
    // 唯一需要动手的那条反而印不出来，索引就失去了它最主要的作用。
    let cand_quota = cand_rows.len().min(lim.saturating_sub(cut_b));
    if !canon_rows.is_empty() {
        println!();
        // 正式记录段跳过已进平面的条目：那两类已经在最上面展开过，
        // 再列一遍既重复又占体积。这里补的是"没进平面、但确实已闭合"的其余记录。
        let in_plane: std::collections::HashSet<String> = plane_dirs
            .iter()
            .chain(plane_rules.iter())
            .map(|(n, _, _)| n.clone())
            .collect();
        let canon_rows: Vec<String> = canon_rows
            .into_iter()
            .filter(|r| {
                // 行里含文件名，取 basename 判是否已在平面。
                !in_plane.iter().any(|n| r.contains(n.as_str()))
            })
            .collect();
        println!("【正式记录】已闭合结论，可直接当依据用");
        let remain_c = lim.saturating_sub(cut_b).saturating_sub(cand_quota);
        let cut_ca = canon_rows.len().min(remain_c);
        let (shown_ca, footer_ca) = crate::paging::page_rows(a, canon_rows[..cut_ca].to_vec());
        for r in &shown_ca {
            println!("{r}");
        }
        if cut_ca < canon_rows.len() {
            println!("  …（另有 {} 条，用 --limit 扩大）", canon_rows.len() - cut_ca);
        }
        if !footer_ca.is_empty() {
            println!("{footer_ca}");
        }
    }

    // 未晋升候选单独成段,并明确标注它的证据地位。
    //
    // 它们与正式记录并列在 Index.md 里,但两者的可用性完全不同:正式库是已闭合
    // 并经用户批准的结论,候选是"可能还有矛盾、还没验证完"的材料。混在一张表里
    // 打印,读者会把候选当成同等可靠的依据——而它只应当用来指方向、不应当当结论。
    // 但也不能不列:不列就等于让模型完全看不见手上还有什么线索。
    //
    // 这一段只装**真没晋升**的那些。已晋升的候选副本只是溯源对象，攒在这里会让
    // 人把"库里有几百条候选"读成"有几百条积压"——实测 299 条候选里 0 条待办，
    // 而旧版这一段会照数报 299 条。
    if !cand_rows.is_empty() {
        println!();
        println!(
            "【未晋升候选】{} 条 —— 仅作参考：可能仍有矛盾或未验证，不可当结论，",
            cand_rows.len()
        );
        println!("确认要用须先读正文核对，或等它升为正式库");
        // 名额是**总共**多少条，不是每段各多少条。各段各自截断会让
        // `--limit 5` 打出 10 行（5 分支 + 5 候选），与"总共 5 条"的直觉不符。
        // 候选在上面的正式段之前就已经分到名额（`cand_quota`），这里直接用。
        let cut_r = cand_rows.len().min(cand_quota);
        let (shown_c, footer_c) = crate::paging::page_rows(a, cand_rows[..cut_r].to_vec());
        for r in &shown_c {
            println!("{r}");
        }
        if cut_r < cand_rows.len() {
            println!("  …（另有 {} 条，用 --limit 扩大）", cand_rows.len() - cut_r);
        }
        if !footer_c.is_empty() {
            println!("{footer_c}");
        }
    }

    // 解析不到单条记录的候选路由（地图文件、`A..B` 区间指针）。单列而不并入
    // 任何一段：它们既不是待办，也不是结论，混进去会让那一段的数字失真。
    if !pointer_rows.is_empty() {
        println!();
        println!("【候选指针】{} 条 —— 地图或区间引用，本身不是单条记录", pointer_rows.len());
        for r in &pointer_rows {
            println!("{r}");
        }
    }

    // 凭据段:只给标识与用途,永不打印值。
    //
    // 这一段的理由不是"顺便列一下"。索引回答的是"我手上有什么牌",而凭据正是
    // 最容易被忘掉的一类牌——记忆里记着某个系统的排障方法,但没人提醒"你还存着
    // 它的账号",于是调用方只会去查资料、不会想到直接去验证。列在这里,调用方
    // 一开始就知道"这个方向上我是可以动手的"。
    let creds = credential_index(&root);
    println!();
    if creds.is_empty() {
        println!("【凭据】0 条（保险箱中无可读条目）");
    } else {
        println!(
            "【凭据】{} 条 —— 只有标识与用途，取值用 abook vault get <标识>",
            creds.len()
        );
        for (id, purpose) in &creds {
            if !q.is_empty() && !ci_contains(id, &q) && !ci_contains(purpose, &q) {
                continue;
            }
            if purpose.is_empty() {
                println!("  {id}");
            } else {
                println!("  {id}｜{purpose}");
            }
        }
    }
    Ok(0)
}

/// 保险箱里可读条目的 (标识, 用途)。
///
/// 名册条目跳过:它列出的是别的条目,把名册本身当成一条凭据列出来会让清单里
/// 多出一条"凭据名册"的假凭据,并且它的用途行会与其余条目的汇总重复一遍。
///
/// 解不开的条目不在这里报——那是 `vault verify` 的职责,索引只回答"有什么"。
/// 但也不能假装它们不存在:调用方按索引取值时才会撞上"这条读不出",所以
/// 索引末尾单独给一行计数,不做静默丢弃。
pub fn credential_index(root: &Path) -> Vec<(String, String)> {
    let (good, bad) = crate::vault::scan_vault(root);
    let mut out: Vec<(String, String)> = good
        .into_iter()
        .filter(|e| e.ident != crate::vault::LEDGER_ID)
        .map(|e| (e.ident, e.purpose))
        .collect();
    out.sort();
    if !bad.is_empty() {
        // 用一条占位让它出现在清单里,读者能看出"还有条目读不出",而不是
        // 以为清单就是全部。
        out.push((
            format!("（另有 {} 条读不出，见 abook vault verify）", bad.len()),
            String::new(),
        ));
    }
    out
}

pub fn cmd_lint(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let libs = libs_from(a);
    let want_json = a.has("json");
    let mut bad = 0;
    let mut checked = 0;
    // Built alongside the human lines rather than in a second pass: lint already
    // has every rule violation in hand, and re-deriving it later is how the two
    // views drift apart.
    let mut rows: Vec<String> = Vec::new();
    for lib in &libs {
        for rec in load_lib(&root, *lib) {
            checked += 1;
            let probs = rec.lint();
            if !probs.is_empty() {
                bad += 1;
                if want_json {
                    rows.push(format!(
                        "{{\"lib\":{},\"file\":{},\"title\":{},\"problems\":[{}]}}",
                        crate::util::jstr(lib.name()),
                        crate::util::jstr(&rec.file_name()),
                        crate::util::jstr(&rec.title()),
                        crate::util::jstr_list(&probs)
                    ));
                } else {
                    println!("[{}] {}  {}", lib.name(), rec.file_name(), rec.title());
                    for pr in &probs {
                        println!("    - {pr}");
                    }
                }
            }
        }
    }
    if want_json {
        println!(
            "{{\"checked\":{checked},\"with_problems\":{bad},\"records\":[{}]}}",
            rows.join(",")
        );
    } else {
        println!("检查 {checked} 条，问题 {bad} 条");
    }
    Ok(if bad > 0 { 5 } else { 0 })
}

pub fn cmd_doctor(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let want_json = a.has("json");
    let mut fail = 0;
    // Collected as structured rows and rendered at the end, so the JSON and the
    // prose come from the same measurements instead of two traversals that can
    // disagree after a file changes in between.
    let mut entries: Vec<(String, String, bool)> = Vec::new();
    if !want_json {
        println!("记忆根    {}", root.display());
    }
    if !root.exists() {
        if want_json {
            println!(
                "{{\"root\":{},\"healthy\":false,\"missing_root\":true,\"entries\":[]}}",
                crate::util::jstr(&root.display().to_string())
            );
        } else {
            println!("  缺失：请先运行 abook init");
        }
        return Ok(4);
    }
    for f in ["Global.md", "Index.md"] {
        let p = root.join(f);
        if p.exists() {
            let n = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            entries.push((f.to_string(), human_bytes(n), true));
        } else {
            entries.push((f.to_string(), "缺失".to_string(), false));
            fail += 1;
        }
    }
    for lib in [Lib::Canonical, Lib::Candidate] {
        let d = root.join(lib.dir());
        let n = list_md(&d).len();
        entries.push((lib.dir().to_string(), format!("{n} 条"), true));
        if !root.join(lib.atlas()).exists() {
            entries.push((lib.atlas().to_string(), "地图缺失".to_string(), false));
            fail += 1;
        }
    }
    let st = read_state(&root);
    for p in &st.problems {
        entries.push(("问题".to_string(), p.clone(), false));
        fail += 1;
    }
    if a.has("fix") {
        // `--fix --dry-run` 必须真的什么都不改。
        //
        // 原来 doctor 的参数表里根本没有 dry-run，于是"预览一下 --fix 会做
        // 什么"这条路径不存在——只能用真实的 `doctor --fix` 去试，而它会
        // 当场重建两张 Atlas 并同步保险箱名册。一个诊断命令的重活没有预演，
        // 等于逼用户拿真库去赌。
        if a.has("dry-run") {
            for lib in [Lib::Canonical, Lib::Candidate] {
                entries.push((
                    lib.atlas().to_string(),
                    "将重建（--dry-run 未改）".to_string(),
                    true,
                ));
            }
            entries.push(("名册".to_string(), "将同步（--dry-run 未改）".to_string(), true));
            if want_json {
                doctor_json(&root, &entries, fail);
            } else {
                println!("[dry-run] 上面两处会重建、名册会同步，本次没有改动任何文件");
            }
            return Ok(if fail > 0 { 5 } else { 0 });
        }
        let _ = fail;
        for lib in [Lib::Canonical, Lib::Candidate] {
            let r = crate::cmd_admin::refresh_atlas(&root, lib, false)?;
            entries.push((lib.atlas().to_string(), format!("已刷新 {} 条路径", r.mapped), true));
        }
        let _ = crate::vault::sync_ledger(&root)?;
        entries.push(("名册".to_string(), "已同步".to_string(), true));
        if want_json {
            doctor_json(&root, &entries, 0);
        }
        return Ok(0);
    }
    if want_json {
        doctor_json(&root, &entries, fail);
        return Ok(if fail > 0 { 5 } else { 0 });
    }
    for (k, v, ok) in &entries {
        println!("  {:<20} {}{}", k, if *ok { "" } else { "  " }, v);
    }
    if fail == 0 {
        println!("结论      健康");
    } else {
        println!("结论      {fail} 个问题（可用 --fix 修复可自动处理项）");
    }
    Ok(if fail > 0 { 5 } else { 0 })
}

/// Render the doctor findings as one document.
fn doctor_json(root: &std::path::Path, entries: &[(String, String, bool)], fail: usize) {
    let rows: Vec<String> = entries
        .iter()
        .map(|(k, v, ok)| {
            format!(
                "{{\"item\":{},\"value\":{},\"ok\":{}}}",
                crate::util::jstr(k),
                crate::util::jstr(v),
                ok
            )
        })
        .collect();
    println!(
        "{{\"root\":{},\"healthy\":{},\"problems\":{fail},\"entries\":[{}]}}",
        crate::util::jstr(&root.display().to_string()),
        fail == 0,
        rows.join(",")
    );
}

pub fn cmd_reconcile(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let mut problems = 0;
    // JSON is assembled alongside the human report; reconcile already computes
    // every number it prints, so one more sink is cheaper than a second pass
    // that could drift from what a human was shown.
    let mut json_libs: Vec<String> = Vec::new();
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
        let disk_n = disk.len();
        let map_n = in_map.len();
        let rec_n = recs.len();
        // Machine output goes through one branch only. Printing the human
        // report first and appending JSON later produced a mixed stream that
        // json.loads() rejects outright.
        let quiet = a.has("json");
        if !quiet {
            println!(
                "== {}  目录 {} / 地图 {} / 已读 {}",
                lib.name(),
                disk_n,
                map_n,
                rec_n
            );
        }
        // Counts are accumulated regardless of output mode so the exit code
        // stays identical for both callers.
        problems += missing.len();
        if !quiet && !missing.is_empty() {
            println!("   地图未收录 {} 条:", missing.len());
            for m in missing.iter().take(10) {
                println!("     + {m}");
            }
        }
        problems += ghost.len();
        if !quiet && !ghost.is_empty() {
            println!("   地图有幽灵条目 {} 条:", ghost.len());
            for g in ghost.iter().take(10) {
                println!("     - {g}");
            }
        }
        // A record belongs to several branches regularly -- "维护与演进" and
        // "凭据保险箱" are both legitimate homes for the same note. Counting
        // those as duplicates made this command announce damage on a healthy
        // library (canonical 2 / candidate 11), which is the worst possible
        // false alarm: it invites "repairing" data that was never broken.
        //
        // So the two cases are separated. The same name twice *within one
        // branch* is a genuine defect -- one of the two lines can never be
        // reached. The same name under different branches is the intended
        // multi-home behaviour and is reported without affecting the verdict.
        let mut dupes: Vec<String> = Vec::new();      // same branch, twice
        for b in &at.branches {
            let mut seen_here: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            for p in b.paths() {
                let base = p.rsplit('/').next().unwrap_or(&p).to_string();
                if !seen_here.insert(base.clone()) && !dupes.contains(&base) {
                    dupes.push(base);
                }
            }
        }
        let mut multi: Vec<String> = Vec::new();       // several branches
        let mut homes: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for b in &at.branches {
            for p in b.paths() {
                let base = p.rsplit('/').next().unwrap_or(&p).to_string();
                let e = homes.entry(base).or_default();
                if !e.contains(&b.heading) {
                    e.push(b.heading.clone());
                }
            }
        }
        for (name, brs) in &homes {
            if brs.len() > 1 {
                multi.push(name.clone());
            }
        }
        // Only the within-branch kind is a problem. Multi-home records make
        // map_n exceed disk_n by design, so the count comparison below uses
        // distinct names instead of raw path count.
        problems += dupes.len();
        if !quiet && !dupes.is_empty() {
            println!("   同一分支内重复 {} 条（真问题，两条只有一条可达）:", dupes.len());
            for d in dupes.iter().take(10) {
                println!("     = {d}");
            }
        }
        if !quiet && !multi.is_empty() {
            println!("   多分支归属 {} 条（正常，同一记录挂在多个分支下）:", multi.len());
            for m in multi.iter().take(10) {
                let brs = homes.get(m).map(|v| v.join("、")).unwrap_or_default();
                println!("     ~ {m}  ->  {brs}");
            }
        }
        let distinct = homes.len();
        if !quiet && missing.is_empty() && ghost.is_empty() && dupes.is_empty() && distinct == disk_n {
            println!("   对账一致");
        }
        if a.has("json") {
            let arr = |v: &Vec<String>| -> String {
                v.iter().map(|x| jstr(x)).collect::<Vec<_>>().join(",")
            };
            let row = format!(
                "{{\"lib\":{},\"disk\":{},\"map\":{},\"distinct\":{},\"parsed\":{},\"missing\":{},\"missing_items\":[{}],\"ghost\":{},\"ghost_items\":[{}],\"duplicates\":{},\"duplicate_items\":[{}],\"multi_branch\":{},\"multi_branch_items\":[{}],\"consistent\":{}}}",
                jstr(lib.name()),
                disk_n,
                map_n,
                distinct,
                rec_n,
                missing.len(),
                arr(&missing),
                ghost.len(),
                arr(&ghost),
                dupes.len(),
                arr(&dupes),
                multi.len(),
                arr(&multi),
                missing.is_empty() && ghost.is_empty() && dupes.is_empty() && distinct == disk_n
            );
            // push, not index-assign: an earlier refactor removed the
            // placeholder push and `len() - 1` underflowed on an empty Vec,
            // panicking (rc=134) instead of emitting JSON.
            json_libs.push(row);
        }
    }
    let (f, r, vp) = crate::vault::reconcile(&root);
    // Count once, before branching: incrementing inside a print loop made the
    // JSON path and the human path disagree on the exit code.
    problems += vp.len();
    if a.has("json") {
        let vault = format!(
            "{{\"files\":{},\"ledger\":{},\"problems\":[{}]}}",
            f,
            r,
            vp.iter().map(|x| jstr(x)).collect::<Vec<_>>().join(",")
        );
        println!(
            "{{\"libraries\":[{}],\"vault\":{vault},\"problems\":{},\"ok\":{}}}",
            json_libs.join(","),
            problems,
            problems == 0
        );
    } else {
        println!("== 保险箱  文件 {f} / 名册 {r}");
        for p in &vp {
            println!("   - {p}");
        }
    }
    Ok(if problems > 0 { 5 } else { 0 })
}

pub fn cmd_recent(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let dir = root.join("commits");
    // Order by the commit's own recorded time, not by mtime.
    //
    // mtime happens to agree today because file writes and events coincide,
    // but a `git clone` or a directory copy resets every mtime at once and
    // would flip this listing wholesale. `history` already reads the declared
    // time; sharing `commit_time` keeps the two commands consistent.
    let mut fs: Vec<(i64, String, Option<u64>, Option<usize>)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            // 读不出来 = 量不出来，不是 0 字节 0 行。
            let raw = read_str(&p).ok();
            let txt = raw.clone().unwrap_or_default();
            fs.push((
                commit_time(&p, &txt),
                p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                raw.as_ref().map(|t| t.len() as u64),
                raw.as_ref().and_then(|t| count_lines_from(t.as_bytes())),
            ));
        }
    }
    fs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    // Read the paging pair through `Page`, not `--limit`: `Args::alias` folds
    // `--limit`/`--size` onto `--per-page`, so looking up "limit" here always
    // missed and the caller got a fixed 10 rows no matter what they asked for.
    let pg = Page::from_args(a, fs.len());
    if a.has("json") {
        let items = fs
            .iter()
            .skip(pg.start())
            .take(pg.per.min(fs.len()))
            .map(|(ts, name, sz, ln)| {
                format!(
                    "{{\"name\":{},{}}}",
                    jstr(name),
                    meta_json(*ts, *sz, *ln)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!("{{{},\"commits\":[{items}]}}", pg.json_fields());
        return Ok(0);
    }
    for (i, (ts, name, sz, ln)) in fs
        .iter()
        .skip(pg.start())
        .take(pg.per.min(fs.len()))
        .enumerate()
    {
        // 元信息前缀从第 0 列起，序号接在后面：`recent` 原来把序号顶在最前，
        // 于是它的三列相对 `list`/`history` 整体右移五个字符，并排看就错位。
        println!(
            "{}  {:>3}. {name}",
            crate::util::row_meta(*ts, *sz, *ln),
            pg.start() + i + 1
        );
    }
    println!("{}", pg.bar());
    Ok(0)
}

pub fn cmd_paths(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    if a.has("json") {
        // Paths are the vocabulary an agent needs before any other call, so
        // machine callers must get them as fields, not as padded text.
        let mut parts: Vec<String> = Vec::new();
        for (k, v) in [
            ("global", root.join("Global.md")),
            ("index", root.join("Index.md")),
            ("canonical", root.join(Lib::Canonical.dir())),
            ("candidate", root.join(Lib::Candidate.dir())),
            ("canonical_atlas", root.join(Lib::Canonical.atlas())),
            ("candidate_atlas", root.join(Lib::Candidate.atlas())),
            ("commits", root.join("commits")),
            ("review", root.join("review")),
            ("staging", root.join("staging")),
            ("vault", root.join("vault")),
        ] {
            parts.push(format!("{}:{}", jstr(k), jstr(&v.display().to_string())));
        }
        println!("{{\"root\":{},\"paths\":{{{}}}}}", jstr(&root.display().to_string()), parts.join(","));
        return Ok(0);
    }
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


#[cfg(test)]
mod tests {
    /// 路由分类只有一套判据，且两条分支不能各说各话。
    ///
    /// 这里钉定的是"文本段"与 JSON 的 `kind` 必须由同一个函数决定；历史上
    /// 一边用 `contains("/records/")`、一边用 `|| ends_with(".md")`，同一条路由
    /// 可能在文本里进"记录"段、在 JSON 里被标成 `branch`。
    #[test]
    fn index_kind_is_one_judgment() {
        // 记录目录下：明确是记录，索引里有没有都不影响。
        assert_eq!(super::index_kind("candidate/records/N-1.md", false), "record");
        assert_eq!(super::index_kind("candidate/records/N-1.md", true), "record");
        // 裸文件名的路由：.md 形状或索引能查到，两者任一成立就是记录。
        // 旧实现里 JSON 侧只认 `/records/`，这一行会返回 `branch`，与文本侧冲突。
        assert_eq!(super::index_kind("N-1.md", false), "record");
        assert_eq!(super::index_kind("N-1.md", true), "record");
        // 不带扩展名、索引里也没有：只能算分支。
        assert_eq!(super::index_kind("N-1", false), "branch");
        assert_eq!(super::index_kind("N-1", true), "record");
        // 分支名。
        assert_eq!(super::index_kind("tools-skill-mcp", false), "branch");
    }
}

/// 预加载平面：把记录分成「用户交代 / 通用强约束 / 经历」三类，前两类列出。
///
/// 为什么需要这一层：`index` 原来只按"知识分支 vs 正式记录"分组，那是**位置**
/// 的分类，不是**作用**的分类。模型看完仍然不知道"用户交代过哪些事非做不可"。
/// 而这类失误的代价最高——不是检索不到，是根本没往那个方向想。
///
/// 体积控制的三条设计：
///   ① 每条只给摘要（SUMMARY 首句，截断到 60 字），不读正文；
///   ② 每类硬上限 DIRECTIVE_MAX / RULE_MAX，超出按最近活跃截断；
///   ③ 截断时**如实报出剩余条数**，不静默丢。
///
/// 经历类不列：它相当于往事档案，用到时才查。把它也铺开会让预加载
/// 随库增长线性膨胀——而这正是这套机制要避免的第一件事。
const DIRECTIVE_MAX: usize = 25;
const RULE_MAX: usize = 18;
const SUMMARY_CHARS: usize = 42;

fn plane_view(
    root: &std::path::Path,
    a: &Args,
) -> (Vec<(String, String, i64)>, Vec<(String, String, i64)>, (usize, usize)) {
    use crate::idx::Plane;
    let mut dirs: Vec<(String, String, i64)> = Vec::new();
    let mut rules: Vec<(String, String, i64)> = Vec::new();

    for lib in [Lib::Canonical] {
        let dir = root.join(lib.dir());
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let raw = match std::fs::read_to_string(&p) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // 只取头部字段，不解析全文。平面视图是"看一眼"的命令，
            // 把 559 条记录全解析成结构体会让它重新变成最贵的一步。
            let field = |name: &str| -> String {
                raw.lines()
                    .find(|l| l.starts_with(name) && l[name.len()..].starts_with('='))
                    .map(|l| l[name.len() + 1..].trim().to_string())
                    .unwrap_or_default()
            };
            let status = field("STATUS");
            if status == "DELETED" || status == "ARCHIVED" {
                continue;
            }
            let kind = field("KIND");
            let authority = field("AUTHORITY");
            let plane = Plane::of(&kind, &authority);
            if plane == Plane::Experience {
                continue;
            }
            // 优先用 BRIEF：那是登记时专门为平面写的一句短话。
            //
            // 没有 BRIEF 才退回 SUMMARY 首句截断——这一步只为兼容存量记录，
            // 截断出来的东西读着是半截话，所以新记录必须写 BRIEF。
            // 优先顺序写死：BRIEF > SUMMARY 首句 > TITLE。
            let summary = {
                let brief = field("BRIEF");
                if !brief.is_empty() {
                    let n = brief.chars().count();
                    if n <= crate::schema::BRIEF_MAX_CHARS {
                        brief
                    } else {
                        // 超长说明登记时没按"一句话"写，如实标出来而不是静默截断：
                        // 静默截断会让作者以为写对了。
                        let cut: String = brief.chars().take(crate::schema::BRIEF_MAX_CHARS).collect();
                        format!("{cut}…")
                    }
                } else {
                    let s = field("SUMMARY");
                    let s = if s.is_empty() { field("TITLE") } else { s };
                    let first = s.split(['。', '；', '\n']).next().unwrap_or(&s).trim().to_string();
                    let n = first.chars().count();
                    if n > SUMMARY_CHARS {
                        let cut: String = first.chars().take(SUMMARY_CHARS).collect();
                        format!("{cut}…")
                    } else {
                        first
                    }
                }
            };
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let ts = e.metadata().ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64).unwrap_or(0);
            if plane == Plane::Directive {
                dirs.push((name, summary, ts));
            } else {
                rules.push((name, summary, ts));
            }
        }
    }
    // 最近活跃优先：截断时留下的是刚更新过的，那些最可能是当前还在用的。
    dirs.sort_by(|a, b| b.2.cmp(&a.2));
    rules.sort_by(|a, b| b.2.cmp(&a.2));
    let d_drop = dirs.len().saturating_sub(DIRECTIVE_MAX);
    let r_drop = rules.len().saturating_sub(RULE_MAX);
    dirs.truncate(DIRECTIVE_MAX);
    rules.truncate(RULE_MAX);
    let _ = a;
    (dirs, rules, (d_drop, r_drop))
}


/// 全量平面（不截断），供 `--plane all` 用。
///
/// 与 `plane_view` 共用同一套分类判据，只是不设上限。分成两个函数而不是
/// 加个 bool 参数，是因为"预加载视图"和"全量清单"是两个用途：
/// 前者体积敏感、后者完整性敏感，混在一起容易在改动时误伤其中一个。
fn plane_view_all(root: &std::path::Path) -> (Vec<(String, String, i64)>, Vec<(String, String, i64)>, (usize, usize)) {
    use crate::idx::Plane;
    let mut dirs: Vec<(String, String, i64)> = Vec::new();
    let mut rules: Vec<(String, String, i64)> = Vec::new();
    let dir = root.join(Lib::Canonical.dir());
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let raw = match std::fs::read_to_string(&p) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let field = |name: &str| -> String {
                raw.lines()
                    .find(|l| l.starts_with(name) && l[name.len()..].starts_with('='))
                    .map(|l| l[name.len() + 1..].trim().to_string())
                    .unwrap_or_default()
            };
            let status = field("STATUS");
            if status == "DELETED" || status == "ARCHIVED" {
                continue;
            }
            let plane = Plane::of(&field("KIND"), &field("AUTHORITY"));
            if plane == Plane::Experience {
                continue;
            }
            let summary = {
                let brief = field("BRIEF");
                if !brief.is_empty() {
                    brief
                } else {
                    let s = field("SUMMARY");
                    let s = if s.is_empty() { field("TITLE") } else { s };
                    let first = s.split(['。', '；', '\n']).next().unwrap_or(&s).trim().to_string();
                    let n = first.chars().count();
                    if n > SUMMARY_CHARS {
                        first.chars().take(SUMMARY_CHARS).collect::<String>() + "…"
                    } else {
                        first
                    }
                }
            };
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let ts = e.metadata().ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64).unwrap_or(0);
            if plane == Plane::Directive { dirs.push((name, summary, ts)); }
            else { rules.push((name, summary, ts)); }
        }
    }
    dirs.sort_by(|a, b| b.2.cmp(&a.2));
    rules.sort_by(|a, b| b.2.cmp(&a.2));
    (dirs, rules, (0, 0))
}
