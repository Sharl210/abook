//! Mutating commands: write (candidate), promote (needs approval), update,
//! delete, restore.

use crate::atlas::{add_to_branch, remove_from_all, Atlas};
use crate::book::BookFile;
use crate::rec::{resolve_record, Lib, Record};
use crate::util::*;
use std::path::Path;

fn opt(a: &Args, k: &str) -> String {
    a.get(k).unwrap_or("").to_string()
}

/// Build a record from CLI flags.
fn build_record(a: &Args, lib: Lib) -> R<Record> {
    let title = a.req("title")?.to_string();
    let body = if let Some(f) = a.get("body-file") {
        read_str(&expand_tilde(f))?
    } else if let Some(b) = a.get("body") {
        b.to_string()
    } else {
        String::new()
    };
    if body.trim().is_empty() {
        return usage("缺少正文：用 --body 或 --body-file 提供");
    }
    let st = stamps();
    let id = Record::new_id(lib, &st.compact, &body);
    let mut rec = Record {
        path: std::path::PathBuf::new(),
        lib,
        fields: Vec::new(),
        body: body.trim_end().to_string(),
        promotion: None,
        raw: String::new(),
    };
    let format = if lib == Lib::Canonical {
        "CANONICAL_MEMORY_V2"
    } else {
        "CANDIDATE_MEMORY_V2"
    };
    rec.set("FORMAT", format);
    rec.set("ID", &id);
    rec.set(
        "PROJECT",
        &{
            let p = opt(a, "project");
            if p.is_empty() { "default".to_string() } else { p }
        },
    );
    rec.set(
        "TASK",
        &{
            let p = opt(a, "task");
            if p.is_empty() { "unspecified".to_string() } else { p }
        },
    );
    rec.set(
        "SESSION",
        &{
            let p = opt(a, "session");
            if p.is_empty() { st.date_only.clone() } else { p }
        },
    );
    rec.set(
        "KIND",
        &{
            let p = opt(a, "kind");
            if p.is_empty() { "OBSERVATION".to_string() } else { p.to_uppercase() }
        },
    );
    rec.set(
        "MATURITY",
        &{
            let p = opt(a, "maturity");
            if p.is_empty() {
                if lib == Lib::Canonical { "VERIFIED".to_string() } else { "OBSERVED".to_string() }
            } else {
                p.to_uppercase()
            }
        },
    );
    rec.set(
        "AUTHORITY",
        &{
            let p = opt(a, "authority");
            if p.is_empty() { "MODEL_DERIVED".to_string() } else { p.to_uppercase() }
        },
    );
    rec.set("TITLE", &title);
    rec.set(
        "SUMMARY",
        &{
            let p = opt(a, "summary");
            if p.is_empty() { truncate_chars(body.trim(), 160) } else { p }
        },
    );
    let read_when = opt(a, "read-when");
    if read_when.is_empty() {
        return usage("缺少 --read-when：将来什么情况下该读这条");
    }
    rec.set("READ_WHEN", &read_when);
    rec.set("NOT_FOR", &opt(a, "not-for"));
    let keys = {
        let mut ks = split_list(&opt(a, "keys"));
        if ks.is_empty() {
            ks = split_list(&title);
        }
        ks.join(",")
    };
    rec.set("KEYS", &keys);
    rec.set("SIZE", &{
        let n = body.len();
        if n < 2048 { "SMALL".to_string() } else if n < 8192 { "MEDIUM".to_string() } else { "LARGE".to_string() }
    });
    let concepts = {
        let c = split_list(&opt(a, "concepts"));
        if c.is_empty() { keys.clone() } else { c.join(",") }
    };
    rec.set("CONCEPTS", &concepts);
    rec.set("ENTITIES", &split_list(&opt(a, "entities")).join(","));
    rec.set("PREVIOUS_ID", &opt(a, "previous-id"));
    rec.set("SUPERSEDES", &opt(a, "supersedes"));
    rec.set("RELATED_IDS", &split_list(&opt(a, "related")).join(","));
    rec.set("EVIDENCE", &opt(a, "evidence"));
    rec.set("UNCERTAINTY", &opt(a, "uncertainty"));
    rec.set("SOURCE", &opt(a, "source"));
    if lib == Lib::Canonical {
        rec.set("STATUS", &{
            let s = opt(a, "status");
            if s.is_empty() { "ACTIVE".to_string() } else { s.to_uppercase() }
        });
    }
    Ok(rec)
}

/// Ensure a record is registered in the atlas under some branch.
fn map_record(root: &Path, rec: &Record, branch_hint: &str) -> R<()> {
    let lib = rec.lib;
    let path = root.join(lib.atlas());
    let mut at = if path.exists() {
        Atlas::load(&path)?
    } else {
        Atlas::default()
    };
    if at.title.is_empty() {
        at.title = format!("{}/Atlas.md", lib.name());
        at.header.push(("FORMAT".into(), "AGENTSBOOK_V2_EXPERIMENT".into()));
        at.header.push(("LIBRARY".into(), lib.name().to_uppercase()));
    }
    let relpath = rel(root, &rec.path);
    let mut branch = if branch_hint.is_empty() {
        crate::query::guess_branch(rec)
    } else {
        branch_hint.to_string()
    };
    if at.branch(&branch).is_none() {
        branch = crate::atlas::DEFAULT_BRANCH.to_string();
    }
    if add_to_branch(&mut at, &branch, &relpath) {
        let total: usize = at.branches.iter().map(|b| b.count()).sum();
        let st = stamps();
        for b in at.branches.iter_mut() {
            if b.name() == branch {
                if let Ok(m) = std::fs::metadata(&rec.path) {
                    b.set("MAXSIZE", &m.len().to_string());
                }
            }
        }
        at.set("GENERATED_AT", &st.iso_z);
        at.set("RECORD_COUNT", &total.to_string());
        at.set("COVERED_THROUGH", &st.iso_z);
        at.set(
            "COVERED_RECORDS",
            &format!("全部{total}条记录；逐条PATH覆盖以本图各分支PATH并集为准"),
        );
        write_atomic(&path, &at.render())?;
    }
    Ok(())
}

fn unmap_record(root: &Path, lib: Lib, relpath: &str) -> R<()> {
    let path = root.join(lib.atlas());
    if !path.exists() {
        return Ok(());
    }
    let mut at = Atlas::load(&path)?;
    let touched = remove_from_all(&mut at, relpath);
    if touched.is_empty() {
        return Ok(());
    }
    let total: usize = at.branches.iter().map(|b| b.count()).sum();
    at.set("RECORD_COUNT", &total.to_string());
    write_atomic(&path, &at.render())
}

fn sync_global_counts(root: &Path) -> R<()> {
    let gp = root.join("Global.md");
    if !gp.exists() {
        return Ok(());
    }
    let mut g = BookFile::load(&gp)?;
    let c = list_md(&root.join(Lib::Canonical.dir())).len();
    let n = list_md(&root.join(Lib::Candidate.dir())).len();
    g.set_state("CANONICAL_RECORDS", &c.to_string());
    g.set_state("CANDIDATE_RECORDS", &n.to_string());
    for lib in [Lib::Canonical, Lib::Candidate] {
        if let Ok(at) = Atlas::load(&root.join(lib.atlas())) {
            let key = if lib == Lib::Canonical {
                "CANONICAL_ATLAS_COVERED_THROUGH"
            } else {
                "CANDIDATE_ATLAS_COVERED_THROUGH"
            };
            g.set_state(key, &at.get("COVERED_THROUGH"));
        }
    }
    write_atomic(&gp, &g.render())
}

pub fn cmd_write(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    crate::cmd_admin::init_book(&root)?;
    let target = a.get("lib").unwrap_or("candidate");
    let lib = if target.starts_with('c') && !target.starts_with("cand") {
        Lib::Canonical
    } else {
        Lib::Candidate
    };
    // canonical writes require explicit approval
    if lib == Lib::Canonical && !a.has("approve") {
        return blocked(
            "正式库写入需要用户明确批准：确认后加 --approve。未批准时请先写入候选库。",
        );
    }
    let mut rec = build_record(a, lib)?;
    let dir = root.join(lib.dir());
    ensure_dir(&dir)?;
    let fname = format!("{}.md", rec.id());
    let path = dir.join(&fname);
    rec.path = path.clone();
    if path.exists() && !a.has("force") {
        return integrity(format!("记录已存在（内容哈希相同视为同一对象）: {fname}"));
    }
    if a.has("dry-run") {
        println!("[dry-run] 将写入 {}", rel(&root, &path));
        println!("{}", rec.render());
        return Ok(0);
    }
    write_atomic(&path, &rec.render())?;
    // read back before reporting success
    let back = Record::parse(&path, lib)?;
    if back.id().is_empty() || back.body != rec.body {
        return integrity("写入复读校验失败");
    }
    map_record(&root, &back, &opt(a, "branch"))?;
    crate::cmd_admin::index_upsert(
        &root,
        &rel(&root, &path),
        &back.title(),
        &back.get("READ_WHEN"),
    )?;
    sync_global_counts(&root)?;
    let body = format!(
        "写入{}记录 {}\n\n标题: {}\nKIND={} MATURITY={}",
        if lib == Lib::Canonical { "正式" } else { "候选" },
        rel(&root, &path),
        back.title(),
        back.kind(),
        back.get("MATURITY")
    );
    crate::cmd_admin::write_commit(&root, "write-record", &body)?;
    println!("{}", rel(&root, &path));
    Ok(0)
}

pub fn cmd_promote(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "候选记录（路径 / 文件名 / ID）")?;
    let (cpath, clib) = resolve_record(&root, sel)?;
    if clib != Lib::Candidate {
        return usage("promote 只接受候选记录");
    }
    let cand = Record::parse(&cpath, Lib::Candidate)?;
    if cand.promotion.is_some() {
        return blocked(format!("该候选已晋升过: {}", cand.file_name()));
    }
    if !a.has("approve") {
        return blocked(format!(
            "晋升需要用户对具体条目的明确批准。记录已找到：「{}」({})。确认后加 --approve。",
            cand.title(),
            rel(&root, &cpath)
        ));
    }
    let st = stamps();
    let id = Record::new_id(Lib::Canonical, &st.compact, &cand.body);
    let mut canon = Record {
        path: std::path::PathBuf::new(),
        lib: Lib::Canonical,
        fields: Vec::new(),
        body: cand.body.clone(),
        promotion: None,
        raw: String::new(),
    };
    canon.set("FORMAT", "CANONICAL_MEMORY_V2");
    canon.set("ID", &id);
    for (k, v) in &cand.fields {
        if matches!(
            k.as_str(),
            "FORMAT" | "ID" | "SEQUENCE" | "PROMOTED_TO" | "PROMOTED_AT"
        ) {
            continue;
        }
        canon.set(k, v);
    }
    canon.set("STATUS", "ACTIVE");
    canon.set("PROMOTED_FROM", &cand.id());
    canon.set("PROMOTED_AT", &st.iso_z);
    // keep a link back to the candidate among related ids
    let related = {
        let mut v = split_list(&cand.get("RELATED_IDS"));
        if !v.iter().any(|x| x == &cand.id()) {
            v.push(cand.id());
        }
        v.join(",")
    };
    canon.set("RELATED_IDS", &related);

    let dir = root.join(Lib::Canonical.dir());
    ensure_dir(&dir)?;
    let path = dir.join(format!("{id}.md"));
    canon.path = path.clone();
    if path.exists() && !a.has("force") {
        return integrity(format!("目标记录已存在: {}", rel(&root, &path)));
    }
    if a.has("dry-run") {
        println!("[dry-run] 将晋升为 {}", rel(&root, &path));
        return Ok(0);
    }
    write_atomic(&path, &canon.render())?;
    let back = Record::parse(&path, Lib::Canonical)?;
    if back.id().is_empty() {
        return integrity("晋升写入复读校验失败");
    }
    map_record(&root, &back, &opt(a, "branch"))?;
    crate::cmd_admin::index_upsert(
        &root,
        &rel(&root, &path),
        &format!("{}（由候选晋升）", back.title()),
        &back.get("READ_WHEN"),
    )?;
    // mark the candidate as promoted
    let mut c2 = cand.clone();
    let promo = format!(
        "PROMOTED_TO={}\nPROMOTED_AT={}\nBRANCH={}\nNOTE=候选正文与头部字段未改写；已晋升候选默认不参与普通自动召回，显式历史查询仍可读取。",
        id,
        st.iso_z,
        opt(a, "branch")
    );
    c2.promotion = Some(promo);
    write_atomic(&cpath, &c2.render())?;
    sync_global_counts(&root)?;
    let body = format!(
        "晋升候选 → 正式\n\n候选: {}\n正式: {}\n标题: {}",
        rel(&root, &cpath),
        rel(&root, &path),
        back.title()
    );
    crate::cmd_admin::write_commit(&root, "promote-record", &body)?;
    println!("{}", rel(&root, &path));
    Ok(0)
}

pub fn cmd_update(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录（路径 / 文件名 / ID）")?;
    let (path, lib) = resolve_record(&root, sel)?;
    let mut rec = Record::parse(&path, lib)?;
    if lib == Lib::Canonical && !a.has("approve") && !a.has("dry-run") {
        return blocked("正式库修改需要用户明确批准：确认后加 --approve。");
    }
    let mut changed = Vec::new();
    for (k, v) in &a.kv {
        let key = match k.to_uppercase().as_str() {
            "TITLE" => "TITLE",
            "SUMMARY" => "SUMMARY",
            "READ-WHEN" | "READ_WHEN" => "READ_WHEN",
            "NOT-FOR" | "NOT_FOR" => "NOT_FOR",
            "KEYS" => "KEYS",
            "CONCEPTS" => "CONCEPTS",
            "ENTITIES" => "ENTITIES",
            "KIND" => "KIND",
            "MATURITY" => "MATURITY",
            "AUTHORITY" => "AUTHORITY",
            "EVIDENCE" => "EVIDENCE",
            "UNCERTAINTY" => "UNCERTAINTY",
            "SOURCE" => "SOURCE",
            "STATUS" => "STATUS",
            "RELATED" => "RELATED_IDS",
            "SUPERSEDES" => "SUPERSEDES",
            "NEXT-VALIDATION" | "NEXT_VALIDATION" => "NEXT_VALIDATION",
            _ => continue,
        };
        let val = v.last().cloned().unwrap_or_default();
        rec.set(key, &val);
        changed.push(key.to_string());
    }
    if let Some(bf) = a.get("body-file") {
        rec.body = read_str(&expand_tilde(bf))?.trim_end().to_string();
        changed.push("BODY".into());
    } else if let Some(b) = a.get("body") {
        rec.body = b.trim_end().to_string();
        changed.push("BODY".into());
    }
    if changed.is_empty() {
        return usage("没有要改的字段");
    }
    if a.has("dry-run") {
        println!("[dry-run] 将修改 {}", rel(&root, &path));
        for c in &changed {
            println!("  {c}");
        }
        return Ok(0);
    }
    // keep ID/link integrity when body changed
    let new_id = Record::new_id(lib, &rec.id(), &rec.body);
    if rec.id().len() == 23 {
        // <prefix>-<15>-<8> already scheme-shaped: keep the filename stable
    } else {
        rec.set("ID", &new_id);
    }
    write_atomic(&path, &rec.render())?;
    let back = Record::parse(&path, lib)?;
    if back.get("TITLE").is_empty() {
        return integrity("修改后复读校验失败");
    }
    crate::cmd_admin::index_upsert(
        &root,
        &rel(&root, &path),
        &back.title(),
        &back.get("READ_WHEN"),
    )?;
    let body = format!(
        "修改记录 {}\n\n字段: {}\n标题: {}",
        rel(&root, &path),
        changed.join(","),
        back.title()
    );
    crate::cmd_admin::write_commit(&root, "update-record", &body)?;
    println!("{}", rel(&root, &path));
    Ok(0)
}

pub fn cmd_delete(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录（路径 / 文件名 / ID）")?;
    let (path, lib) = resolve_record(&root, sel)?;
    let rec = Record::parse(&path, lib)?;
    if !a.has("approve") {
        return blocked("删除需要用户明确批准：确认后加 --approve。不带 --hard 时可恢复。");
    }
    if a.has("dry-run") {
        println!("[dry-run] 将删除 {}", rel(&root, &path));
        return Ok(0);
    }
    if a.has("hard") {
        std::fs::remove_file(&path)?;
    } else {
        let mut r2 = rec.clone();
        r2.set("STATUS", "DELETED");
        write_atomic(&path, &r2.render())?;
    }
    unmap_record(&root, lib, &rel(&root, &path))?;
    crate::cmd_admin::index_remove(&root, &rel(&root, &path))?;
    sync_global_counts(&root)?;
    let body = format!(
        "删除记录 {}\n\n模式: {}\n标题: {}",
        rel(&root, &path),
        if a.has("hard") { "物理删除" } else { "软删除(STATUS=DELETED)" },
        rec.title()
    );
    crate::cmd_admin::write_commit(&root, "delete-record", &body)?;
    println!("{}", rel(&root, &path));
    Ok(0)
}

pub fn cmd_restore(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录（路径 / 文件名 / ID）")?;
    let (path, lib) = resolve_record(&root, sel)?;
    let mut rec = Record::parse(&path, lib)?;
    if lib == Lib::Canonical && !a.has("approve") && !a.has("dry-run") {
        return blocked("正式库恢复需要用户明确批准：确认后加 --approve。");
    }
    if rec.status() == "ACTIVE" {
        println!("已处于 ACTIVE: {}", rel(&root, &path));
        return Ok(0);
    }
    if a.has("dry-run") {
        println!("[dry-run] 将恢复 {}", rel(&root, &path));
        return Ok(0);
    }
    rec.set("STATUS", "ACTIVE");
    write_atomic(&path, &rec.render())?;
    let back = Record::parse(&path, lib)?;
    map_record(&root, &back, &opt(a, "branch"))?;
    crate::cmd_admin::index_upsert(
        &root,
        &rel(&root, &path),
        &back.title(),
        &back.get("READ_WHEN"),
    )?;
    sync_global_counts(&root)?;
    let body = format!("恢复记录 {}\n\n标题: {}", rel(&root, &path), back.title());
    crate::cmd_admin::write_commit(&root, "restore-record", &body)?;
    println!("{}", rel(&root, &path));
    Ok(0)
}
