//! Fine-grained record editing: single-field set, body append/prepend,
//! text patch, rename, branch move, merge, split, copy, demote.
//!
//! Every write goes through the same chain: parse tolerantly, validate, write
//! to a temp file, read back, compare, atomically rename, then update atlas,
//! index, Global counts and the commit log.

use crate::rec::{resolve_record, Lib, Record};
use crate::schema;
use crate::util::*;
use std::path::Path;

fn guard(a: &Args, lib: Lib, what: &str) -> R<()> {
    if lib == Lib::Canonical && !a.has("approve") && !a.has("dry-run") {
        return blocked(format!("正式库{what}需要用户明确批准：确认后加 --approve。"));
    }
    Ok(())
}

/// Persist one record and keep every derived artefact in step.
pub fn save(root: &Path, rec: &mut Record, a: &Args, op: &str) -> R<()> {
    let body = rec.render();
    // validate what we are about to write
    let parsed = crate::parse::parse(&body, Some(rec.lib));
    if parsed.has_error() {
        let msgs: Vec<String> = parsed.errors().iter().map(|d| d.show()).collect();
        return integrity(format!("待写入内容未通过解析: {}", msgs.join(" | ")));
    }
    let rep = schema::validate(&parsed, rec.lib, Some(&rec.file_name()));
    if rep.has_error() {
        let msgs: Vec<String> = rep
            .issues
            .iter()
            .filter(|i| i.sev == crate::parse::Sev::Error)
            .map(|i| i.show())
            .collect();
        return integrity(format!("待写入内容未通过校验: {}", msgs.join(" | ")));
    }
    if a.has("dry-run") {
        println!("[dry-run] {op} {}", rel(root, &rec.path));
        println!("{body}");
        return Ok(());
    }
    write_atomic(&rec.path, &body)?;
    let back = Record::parse(&rec.path, rec.lib)?;
    if back.body != rec.body && !rec.body.trim().is_empty() {
        return integrity(format!(
            "写入复读不一致: {}",
            rel(root, &rec.path)
        ));
    }
    // atlas + index + counts
    crate::cmd_write_sync::sync_record(root, &back)?;
    crate::cmd_admin::write_commit(
        root,
        op,
        &format!("{op}\n\n记录: {}\n标题: {}", rel(root, &rec.path), back.title()),
    )?;
    Ok(())
}

pub fn cmd_set(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let field = a.need_pos(1, "字段名")?;
    let value = a.pos.get(2).cloned().unwrap_or_else(|| {
        a.get("value").unwrap_or("").to_string()
    });
    let (path, lib) = resolve_record(&root, sel)?;
    guard(a, lib, "修改")?;
    let mut rec = Record::parse(&path, lib)?;
    let (canon, _) = crate::parse::canon_key(&field);
    if canon.is_empty() {
        return usage(format!("字段名不合法: {field}"));
    }
    rec.set(&canon, &value);
    save(&root, &mut rec, a, "set-field")?;
    if !a.has("dry-run") {
        println!("{}= {}", canon, value);
    }
    Ok(0)
}

pub fn cmd_unset(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let field = a.need_pos(1, "字段名")?;
    let (path, lib) = resolve_record(&root, sel)?;
    guard(a, lib, "修改")?;
    let mut rec = Record::parse(&path, lib)?;
    let (canon, _) = crate::parse::canon_key(&field);
    rec.fields.retain(|(k, _)| k != &canon);
    save(&root, &mut rec, a, "unset-field")?;
    println!("已移除 {canon}");
    Ok(0)
}

fn read_payload(a: &Args) -> R<String> {
    if let Some(f) = a.get("file") {
        return read_str(&expand_tilde(f));
    }
    if let Some(t) = a.get("text") {
        return Ok(t.to_string());
    }
    if let Some(b) = a.get("body") {
        return Ok(b.to_string());
    }
    let mut s = String::new();
    use std::io::Read;
    std::io::stdin().read_to_string(&mut s)?;
    Ok(s)
}

pub fn cmd_append(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let (path, lib) = resolve_record(&root, sel)?;
    guard(a, lib, "追加正文")?;
    let mut rec = Record::parse(&path, lib)?;
    let payload = read_payload(a)?;
    if payload.trim().is_empty() {
        return usage("没有要追加的内容");
    }
    let old_body = rec.body.clone();
    if !rec.body.is_empty() && !rec.body.ends_with('\n') {
        rec.body.push('\n');
    }
    rec.body.push_str(payload.trim_end());
    rederive_after_body_change(&mut rec, &old_body);
    save(&root, &mut rec, a, "append-body")?;
    if !a.has("dry-run") {
        println!("正文已追加，现 {} 字节", rec.body.len());
    }
    Ok(0)
}

pub fn cmd_prepend(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let (path, lib) = resolve_record(&root, sel)?;
    guard(a, lib, "前置正文")?;
    let mut rec = Record::parse(&path, lib)?;
    let payload = read_payload(a)?;
    if payload.trim().is_empty() {
        return usage("没有要插入的内容");
    }
    let old_body = rec.body.clone();
    let mut s = payload.trim_end().to_string();
    s.push('\n');
    s.push_str(&rec.body);
    rec.body = s;
    rederive_after_body_change(&mut rec, &old_body);
    save(&root, &mut rec, a, "prepend-body")?;
    if !a.has("dry-run") {
        println!("正文已前置，现 {} 字节", rec.body.len());
    }
    Ok(0)
}

pub fn cmd_patch(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let (path, lib) = resolve_record(&root, sel)?;
    guard(a, lib, "替换")?;
    let old = a.get("old").ok_or_else(|| EE::Usage("缺 --old".into()))?.to_string();
    let new = a.get("new").unwrap_or("").to_string();
    let mut rec = Record::parse(&path, lib)?;
    let scope = a.get("in").unwrap_or("body");
    let hay = if scope == "field" {
        a.get("field").unwrap_or("").to_string()
    } else {
        "body".to_string()
    };
    let mut hits = 0usize;
    if hay == "body" {
        let old_body = rec.body.clone();
        hits = rec.body.matches(&old).count();
        if hits == 0 {
            return notfound("正文中未找到该文本");
        }
        if hits > 1 && !a.has("all") {
            return usage(format!("命中 {hits} 处，确认全部替换请加 --all"));
        }
        rec.body = rec.body.replace(&old, &new);
        rederive_after_body_change(&mut rec, &old_body);
    } else {
        let cur = rec.get(&hay);
        hits = cur.matches(&old).count();
        if hits == 0 {
            return notfound(format!("字段 {hay} 中未找到该文本"));
        }
        rec.set(&hay, &cur.replace(&old, &new));
    }
    save(&root, &mut rec, a, "patch-text")?;
    if !a.has("dry-run") {
        println!("已替换 {hits} 处");
    }
    Ok(0)
}

pub fn cmd_rename(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let newname = a.need_pos(1, "新文件名或新标题")?;
    let (path, lib) = resolve_record(&root, sel)?;
    guard(a, lib, "改名")?;
    let mut rec = Record::parse(&path, lib)?;
    if a.has("title") {
        rec.set("TITLE", &newname);
        save(&root, &mut rec, a, "retitle")?;
        println!("标题已改为 {newname}");
        return Ok(0);
    }
    let clean: String = if newname.ends_with(".md") {
        newname.to_string()
    } else {
        format!("{newname}.md")
    };
    let dst = path.with_file_name(&clean);
    if dst.exists() {
        return integrity(format!("目标已存在: {}", rel(&root, &dst)));
    }
    if a.has("dry-run") {
        println!("[dry-run] {} → {}", rel(&root, &path), rel(&root, &dst));
        return Ok(0);
    }
    let old_rel = rel(&root, &path);
    std::fs::rename(&path, &dst)?;
    rec.path = dst.clone();
    // keep the header ID aligned with the new name when it was aligned before
    if Record::is_scheme_name(&clean) {
        let stem = clean.trim_end_matches(".md").to_string();
        rec.set("ID", &stem);
        write_atomic(&dst, &rec.render())?;
    }
    crate::cmd_write_sync::repoint(&root, lib, &old_rel, &rel(&root, &dst))?;
    crate::cmd_admin::write_commit(
        &root,
        "rename-record",
        &format!("改名\n\n旧: {old_rel}\n新: {}", rel(&root, &dst)),
    )?;
    println!("{}", rel(&root, &dst));
    Ok(0)
}

pub fn cmd_move(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let branch = a.need_pos(1, "目标分支")?;
    let (path, lib) = resolve_record(&root, sel)?;
    guard(a, lib, "换分支")?;
    if a.has("dry-run") {
        println!("[dry-run] 把 {} 移到分支 {branch}", rel(&root, &path));
        return Ok(0);
    }
    crate::cmd_write_sync::move_branch(&root, lib, &rel(&root, &path), &branch)?;
    crate::cmd_admin::write_commit(
        &root,
        "move-branch",
        &format!("换分支\n\n记录: {}\n新分支: {branch}", rel(&root, &path)),
    )?;
    println!("已移到分支 {branch}");
    Ok(0)
}

pub fn cmd_copy(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let (path, lib) = resolve_record(&root, sel)?;
    let mut rec = Record::parse(&path, lib)?;
    let target_lib = match a.get("to").unwrap_or("") {
        "canonical" | "c" => Lib::Canonical,
        "candidate" | "n" => Lib::Candidate,
        _ => lib,
    };
    if target_lib == Lib::Canonical && !a.has("approve") && !a.has("dry-run") {
        return blocked("复制到正式库需要 --approve。");
    }
    let st = stamps();
    let id = Record::new_id(target_lib, &st.compact, &rec.body);
    rec.set("ID", &id);
    rec.set(
        "FORMAT",
        if target_lib == Lib::Canonical {
            "CANONICAL_MEMORY_V2"
        } else {
            "CANDIDATE_MEMORY_V2"
        },
    );
    let dir = root.join(target_lib.dir());
    ensure_dir(&dir)?;
    let dst = dir.join(format!("{id}.md"));
    rec.path = dst.clone();
    rec.lib = target_lib;
    save(&root, &mut rec, a, "copy-record")?;
    if !a.has("dry-run") {
        println!("{}", rel(&root, &dst));
    }
    Ok(0)
}

pub fn cmd_demote(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "正式记录")?;
    let (path, lib) = resolve_record(&root, sel)?;
    if lib != Lib::Canonical {
        return usage("demote 只接受正式记录");
    }
    if !a.has("approve") && !a.has("dry-run") {
        return blocked("把正式记录退回候选需要 --approve。");
    }
    let mut rec = Record::parse(&path, lib)?;
    let st = stamps();
    let id = Record::new_id(Lib::Candidate, &st.compact, &rec.body);
    rec.set("ID", &id);
    rec.set("FORMAT", "CANDIDATE_MEMORY_V2");
    rec.set("STATUS", "ACTIVE");
    rec.set(
        "CANDIDATE_SOURCE",
        &format!("从正式记录退回: {}", rec.file_name()),
    );
    let dir = root.join(Lib::Candidate.dir());
    ensure_dir(&dir)?;
    let dst = dir.join(format!("{id}.md"));
    rec.path = dst.clone();
    rec.lib = Lib::Candidate;
    save(&root, &mut rec, a, "demote-record")?;
    // remove the canonical copy
    if !a.has("dry-run") {
        let old_rel = rel(&root, &path);
        std::fs::remove_file(&path)?;
        crate::cmd_write_sync::forget(&root, Lib::Canonical, &old_rel)?;
        println!("{}", rel(&root, &dst));
    }
    Ok(0)
}

pub fn cmd_merge(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    if a.pos.len() < 2 {
        return usage("merge 需要至少两条记录");
    }
    let mut recs: Vec<(std::path::PathBuf, Lib, Record)> = Vec::new();
    for sel in &a.pos {
        let (p, l) = resolve_record(&root, sel)?;
        let r = Record::parse(&p, l)?;
        recs.push((p, l, r));
    }
    let lib = recs[0].1;
    if lib == Lib::Canonical && !a.has("approve") && !a.has("dry-run") {
        return blocked("合并进正式库需要 --approve。");
    }
    let title = a
        .get("title")
        .map(|s| s.to_string())
        .unwrap_or_else(|| recs[0].2.title());
    let mut body = String::new();
    for (_, _, r) in &recs {
        body.push_str(&format!("## 来自 {}\n\n", r.file_name()));
        body.push_str(r.body.trim_end());
        body.push_str("\n\n");
    }
    let st = stamps();
    let id = Record::new_id(lib, &st.compact, &body);
    // union the recall keys so the merged record stays findable
    let mut keys: Vec<String> = Vec::new();
    for (_, _, r) in &recs {
        for part in r.get("KEYS").split([',', '，']) {
            let t = part.trim();
            if !t.is_empty() && !keys.iter().any(|k| k == t) {
                keys.push(t.to_string());
            }
        }
    }
    let mut out = Record {
        path: root.join(lib.dir()).join(format!("{id}.md")),
        lib,
        fields: Vec::new(),
        body: body.trim_end().to_string(),
        promotion: None,
        raw: String::new(),
        devs: Vec::new(),
        header_inferred: false,
    };
    out.set(
        "FORMAT",
        if lib == Lib::Canonical {
            "CANONICAL_MEMORY_V2"
        } else {
            "CANDIDATE_MEMORY_V2"
        },
    );
    out.set("ID", &id);
    out.set("KIND", &recs[0].2.kind());
    out.set("MATURITY", &recs[0].2.get("MATURITY"));
    out.set("AUTHORITY", &recs[0].2.get("AUTHORITY"));
    out.set("TITLE", &title);
    out.set("SUMMARY", &format!("合并 {} 条记录", recs.len()));
    out.set(
        "READ_WHEN",
        &recs[0].2.get("READ_WHEN"),
    );
    out.set("NOT_FOR", &recs[0].2.get("NOT_FOR"));
    out.set("KEYS", &keys.join(","));
    out.set("CONCEPTS", &keys.join(","));
    out.set(
        "RELATED_IDS",
        &recs
            .iter()
            .map(|(_, _, r)| r.id())
            .collect::<Vec<_>>()
            .join(","),
    );
    out.set(
        "SOURCE",
        &format!("merge: {}", recs.iter().map(|(_, _, r)| r.file_name()).collect::<Vec<_>>().join(" + ")),
    );
    ensure_dir(&root.join(lib.dir()))?;
    save(&root, &mut out, a, "merge-records")?;
    if !a.has("dry-run") {
        println!("{}", rel(&root, &out.path));
        if a.has("remove-sources") {
            for (p, l, _) in &recs {
                let r = rel(&root, p);
                let mut rr = Record::parse(p, *l)?;
                rr.set("STATUS", "SUPERSEDED");
                rr.set("SUPERSEDES", &id);
                write_atomic(p, &rr.render())?;
                crate::cmd_write_sync::forget(&root, *l, &r)?;
            }
            println!("源记录已标记 SUPERSEDED");
        }
    }
    Ok(0)
}

pub fn cmd_split(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let (path, lib) = resolve_record(&root, sel)?;
    guard(a, lib, "拆分")?;
    let rec = Record::parse(&path, lib)?;
    let marker = a
        .get("at")
        .ok_or_else(|| EE::Usage("缺 --at <标题行文本>".into()))?
        .to_string();
    let mut pre: Vec<&str> = Vec::new();
    let mut post: Vec<&str> = Vec::new();
    let mut seen = false;
    for line in rec.body.lines() {
        if !seen && line.contains(&marker) {
            seen = true;
        }
        if seen {
            post.push(line);
        } else {
            pre.push(line);
        }
    }
    if !seen {
        return notfound(format!("正文中未找到分隔文本: {marker}"));
    }
    if pre.is_empty() || post.is_empty() {
        return usage("拆分后会出现空段，请换一个分隔位置");
    }
    if a.has("dry-run") {
        println!("[dry-run] 拆成两段: {} 行 / {} 行", pre.len(), post.len());
        return Ok(0);
    }
    let st = stamps();
    let body2 = post.join("\n");
    let id2 = Record::new_id(lib, &st.compact, &body2);
    let mut r2 = rec.clone();
    r2.body = body2;
    r2.set("ID", &id2);
    r2.set("TITLE", &format!("{}（续）", rec.title()));
    r2.path = root.join(lib.dir()).join(format!("{id2}.md"));
    r2.promotion = None;
    save(&root, &mut r2, a, "split-record")?;
    let mut r1 = rec.clone();
    r1.body = pre.join("\n");
    r1.set("RELATED_IDS", &id2);
    save(&root, &mut r1, a, "split-record")?;
    println!("{}", rel(&root, &r2.path));
    Ok(0)
}
