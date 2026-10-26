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
    let mut rec = Record {
        path: std::path::PathBuf::new(),
        lib,
        fields: Vec::new(),
        body: body.trim_end().to_string(),
        promotion: None,
        attach: None,
        raw: String::new(),
        devs: Vec::new(),
        header_inferred: false,
        // A brand-new record has no bytes on disk yet.
        base: None,
        // Nothing is on disk to stat, so it counts as touched right now --
        // which is true, and keeps a fresh record at the top of a newest-first
        // listing instead of sorting as "never".
        mtime: crate::util::now_secs(),
    };
    let format = if lib == Lib::Canonical {
        "CANONICAL_MEMORY_V2"
    } else {
        "CANDIDATE_MEMORY_V2"
    };
    rec.set("FORMAT", format);
    // A brand-new record has no `mtime` to inherit and no earlier `UPDATED`,
    // so stamp it once here. Without this, every freshly written record would
    // sort as "never touched" and land at the bottom of a recency listing --
    // exactly backwards, since it is the newest thing in the library.
    rec.set("UPDATED", &now_iso_local());
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
            if p.is_empty() {
                derive_summary(&body)
            } else {
                p
            }
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
    // Derive the ID last: it hashes the whole record, so every field above
    // must already be in place.
    let id = rec.id_for(&st.compact);
    rec.set("ID", &id);
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
        // `total_count()` de-duplicates by record name. Summing per-branch
        // COUNT fields is wrong: one record may be routed from several
        // branches, so the sum over-counts (the shipped library reads 202
        // that way while only 188 records exist). The library's own headers
        // use the unique count, so this must match it.
        let total: usize = at.total_count();
        let st = stamps();
        // MAXSIZE is the largest record in the branch, not the newest one.
        // Assigning it unconditionally made the field shrink every time a
        // small record was added.
        for b in at.branches.iter_mut() {
            if b.name() == branch {
                if let Ok(m) = std::fs::metadata(&rec.path) {
                    let prev = b.get("MAXSIZE").parse::<u64>().unwrap_or(0);
                    if m.len() > prev {
                        b.set("MAXSIZE", &m.len().to_string());
                    }
                }
            }
        }
        at.set("GENERATED_AT", &st.iso_z);
        at.set("RECORD_COUNT", &total.to_string());
        at.set("COVERED_THROUGH", &st.iso_z);
        at.set(
            "COVERED_RECORDS",
            &format!("全部{total}条记录；逐条PATH覆盖以本图各分支PATH并集为准（已对账{total}/{total}）"),
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
    let total: usize = at.total_count();
    at.set("RECORD_COUNT", &total.to_string());
    write_atomic(&path, &at.render())
}

fn sync_global_counts(root: &Path) -> R<()> {
    // One implementation for every write path. This used to be a second copy that
    // counted with `list_md` and wrote only `Global.md`'s body, so a count could be
    // right by one rule and wrong by the other depending on which command ran.
    crate::cmd_write_sync::sync_counts(root)
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
        crate::say_line!("[dry-run] 将写入 {}", rel(&root, &path));
        crate::say_line!("{}", rec.render());
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
    crate::cmd_admin::write_commit(
        &root,
        &crate::cmd_admin::CommitInfo {
            slug: "write-record",
            scope: "仅新增 1 条记录，不改动既有记录",
            operation: "CREATE",
            library: if lib == Lib::Canonical { "CANONICAL" } else { "CANDIDATE" },
            summary: &format!("新建{}记录 {}", if lib == Lib::Canonical { "正式" } else { "候选" }, rel(&root, &path)),
            target: &rel(&root, &path),
            body: &body,
        },
    )?;
    crate::say_line!("{}", rel(&root, &path));
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

    // 已晋升的候选不许再晋升一次。
    //
    // 两个地方都要看，因为它们来自不同时期：新流程写末尾的 `[PROMOTION]` 块
    // （`cand.promotion`），而早期流程把 `PROMOTED_TO`/`PROMOTED_AT` 直接写在
    // 头部字段里。只查前者时，一条头部已标 `PROMOTED_TO=C-...` 的老候选会被
    // 当成"没晋升过"再晋升一遍，正式库于是出现两条标题相同、`PROMOTED_FROM`
    // 也相同的记录——重复就是这么来的。
    if cand.promotion.is_some() {
        return blocked(format!("该候选已晋升过: {}", cand.file_name()));
    }
    let hdr_to = cand.get("PROMOTED_TO").trim().to_string();
    if !hdr_to.is_empty() {
        let dest = if root.join("canonical/records").join(format!("{hdr_to}.md")).exists() {
            "正式库里还在"
        } else {
            "该正式记录已不在库里"
        };
        return blocked(format!(
            "该候选头部已记 PROMOTED_TO={hdr_to}（{dest}），说明此前晋升过。\n\
             要重新晋升请先确认那次晋升是否作废：作废就 `abook set {} PROMOTED_TO \"\"` 清掉再试；\n\
             只是想改已晋升内容，直接编辑正式记录 {}。",
            cand.file_name(),
            hdr_to
        ));
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
        attach: None,
        raw: String::new(),
        devs: Vec::new(),
        header_inferred: false,
        // A newly promoted record does not exist on disk yet.
        base: None,
        // Inherit the candidate's timestamp on purpose: promotion is a move,
        // not an edit, so the promoted copy should keep showing when it was
        // actually learned. Stamping it `now` would make every promotion look
        // like new knowledge and bury what it was derived from.
        mtime: cand.mtime.max(cand.created()),
    };
    canon.set("FORMAT", "CANONICAL_MEMORY_V2");
    // Inherit the candidate's timestamp: promotion moves a record, it does not
    // edit it, so the promoted copy should keep showing when it was actually
    // learned. Stamping `now` here would make every promotion look like brand
    // new knowledge and bury what it was derived from.
    let inherited = {
        let u = cand.updated();
        if u > 0 { u } else { now_secs() }
    };
    canon.set("UPDATED", &crate::util::stamps_at(inherited).iso_local);
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
        crate::say_line!("[dry-run] 将晋升为 {}", rel(&root, &path));
        return Ok(0);
    }
    write_atomic(&path, &canon.render())?;
    let back = Record::parse(&path, Lib::Canonical)?;
    if back.id().is_empty() {
        return integrity("晋升写入复读校验失败");
    }
    // 附件随晋升一起走。
    //
    // 晋升是"搬"不是"抄"：正文原样搬过去并保持原时间戳，附件目录也必须跟着
    // 改名搬过去。否则晋升后正文还在、它携带的截图与日志却留在候选侧，候选
    // 一旦清理证据就没了——一条只剩正文、丢了证据的记录，比没有这条记录更
    // 容易误导人。
    let mut moved_attach = String::new();
    let cand_dirs = crate::attach::all_dirs(&cpath);
    if !cand_dirs.is_empty() {
        let want = crate::attach::dir_for(&path);
        if want.exists() {
            return blocked(format!(
                "正式侧已存在附件目录 {}。先把两边文件核对清楚，再决定合并还是改名",
                rel(&root, &want)
            ));
        }
        let (mut moved_n, mut moved_b) = (0usize, 0u64);
        for d in &cand_dirs {
            if let Ok(rd) = std::fs::read_dir(d) {
                for e in rd.flatten() {
                    if let Ok(m) = std::fs::metadata(e.path()) {
                        moved_n += 1;
                        moved_b += m.len();
                    }
                }
            }
        }
        // 第一个目录整目录改名（同设备上零拷贝），其余手建旧命名的内容并入。
        for (i, d) in cand_dirs.iter().enumerate() {
            if i == 0 {
                std::fs::rename(d, &want)?;
                continue;
            }
            for e in std::fs::read_dir(d)?.flatten() {
                let from = e.path();
                let to = want.join(e.file_name());
                if to.exists() {
                    return blocked(format!(
                        "附件重名 {}：{} 与正式侧已有文件冲突，先手工处理",
                        e.file_name().to_string_lossy(),
                        rel(&root, &from)
                    ));
                }
                std::fs::rename(&from, &to)?;
            }
            let _ = std::fs::remove_dir(d);
        }
        moved_attach = format!(
            "附件 {} 个（{}）已随记录搬到 {}",
            moved_n,
            human_bytes(moved_b),
            rel(&root, &want)
        );
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
        "晋升候选 → 正式\n\n候选: {}\n正式: {}\n标题: {}{}",
        rel(&root, &cpath),
        rel(&root, &path),
        back.title(),
        if moved_attach.is_empty() {
            String::new()
        } else {
            format!("\n{moved_attach}")
        }
    );
    crate::cmd_admin::write_commit(
        &root,
        &crate::cmd_admin::CommitInfo {
            slug: "promote-record",
            scope: "候选 1 条移入正式库，正式库计数与双地图同步",
            operation: "REPLACE",
            library: "BOTH",
            summary: &format!("候选晋升为正式 {sel}"),
            target: "canonical/records",
            body: &body,
        },
    )?;
    crate::say_line!("{}", rel(&root, &path));
    if !moved_attach.is_empty() {
        crate::say_line!("{moved_attach}");
    }
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
        crate::say_line!("[dry-run] 将修改 {}", rel(&root, &path));
        for c in &changed {
            crate::say_line!("  {c}");
        }
        return Ok(0);
    }
    // ID 是身份，不随正文改动而变。
    //
    // 初版在这里按"正文变了就换一个新 ID"处理，并且用 `rec.id().len() == 23`
    // 判断"是否已是规范形态"。真实的规范 ID 是 26 字符（`N-` + 15 位时间 +
    // `-` + 8 位哈希），于是这个条件**永远不成立**，每次 update 都把 ID 改成
    // `N-<旧ID>-<新哈希>`——一条记录被编辑两次就套成 `N-N-...`。
    //
    // 危害不只是难看：ID 是 `RELATED_IDS`、`PROMOTED_FROM` 等所有引用的指向
    // 目标，改它等于把入向引用全部打断；而文件名没变，于是 ID 与文件名也分叉了。
    //
    // 正确做法：正文编辑不动 ID。只有 ID 缺失或不成形（旧库手写记录）时，
    // 才对齐到文件名干——因为文件名才是磁盘上的规范身份。
    let shaped = Record::is_scheme_name(&format!("{}.md", rec.id()));
    if !shaped {
        if let Some(stem) = path.file_stem().map(|x| x.to_string_lossy().to_string()) {
            if Record::is_scheme_name(&format!("{stem}.md")) {
                rec.set("ID", &stem);
            }
        }
    }
    // `update` replaces field values wholesale, so an agent that read the record
    // minutes ago and edits from memory would silently revert anything written in
    // between. The fingerprint it was shown is the only evidence of what it
    // actually read.
    if let Some(want) = a.get("if-version") {
        require_version(&path, want)?;
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
    crate::cmd_admin::write_commit(
        &root,
        &crate::cmd_admin::CommitInfo {
            slug: "update-record",
            scope: "仅改写 1 条记录，头部与地图按需同步",
            operation: "PATCH",
            library: "CANONICAL",
            summary: &format!("改写记录 {sel}"),
            target: "canonical/records",
            body: &body,
        },
    )?;
    crate::say_line!("{}", rel(&root, &path));
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
        crate::say_line!("[dry-run] 将删除 {}", rel(&root, &path));
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
    crate::cmd_admin::write_commit(
        &root,
        &crate::cmd_admin::CommitInfo {
            slug: "delete-record",
            scope: "1 条记录转为删除态，不物理删除",
            operation: "DELETE",
            library: "CANONICAL",
            summary: &format!("删除记录 {sel}"),
            target: "canonical/records",
            body: &body,
        },
    )?;
    crate::say_line!("{}", rel(&root, &path));
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
        crate::say_line!("已处于 ACTIVE: {}", rel(&root, &path));
        return Ok(0);
    }
    if a.has("dry-run") {
        crate::say_line!("[dry-run] 将恢复 {}", rel(&root, &path));
        return Ok(0);
    }
    rec.set("STATUS", "ACTIVE");
    // Reactivating is an edit like any other, so it counts as touched.
    rec.set("UPDATED", &now_iso_local());
    if let Some(want) = a.get("if-version") {
        require_version(&path, want)?;
    }
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
    crate::cmd_admin::write_commit(
        &root,
        &crate::cmd_admin::CommitInfo {
            slug: "restore-record",
            scope: "1 条记录恢复为可见态",
            operation: "RESTORE",
            library: "CANONICAL",
            summary: &format!("恢复记录 {sel}"),
            target: "canonical/records",
            body: &body,
        },
    )?;
    crate::say_line!("{}", rel(&root, &path));
    Ok(0)
}
