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

/// Render the dry-run report. Kept separate from the writing path so `selftest`
/// can assert the contract: a dry run is a decision aid, so it stays short,
/// and the full text is available only on request.
pub fn render_dry_run(rec: &Record, op: &str, target: &str, verbose: bool) -> String {
    let mut out = format!("[dry-run] {op} {target}\n");
    out.push_str(&format!(
        "          字段 {} 个，正文 {} 字节（校验已通过；加 --verbose 可看全文）\n",
        rec.fields.len(),
        rec.body.len()
    ));
    if verbose {
        out.push_str(&rec.render());
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Same function under a name that says why `selftest` calls it.
pub fn render_dry_run_for_test(rec: &Record, op: &str, target: &str, verbose: bool) -> String {
    render_dry_run(rec, op, target, verbose)
}

/// Write the record back. Returns `true` when bytes actually changed.
///
/// The `bool` matters: a same-value edit used to write the file, rewrite the
/// map, bump the index and append a change record that announced a change which
/// never happened. Re-running a bulk "normalise" pass over the library
/// therefore inflated it by hundreds of phantom commits -- 286 of them in one
/// test pass. A write that changes nothing must not claim to have changed
/// anything.
pub fn save(root: &Path, rec: &mut Record, a: &Args, op: &str) -> R<bool> {
    // Decide whether anything really changed *before* stamping the time.
    //
    // Stamping first made every no-op edit look like a change, because the
    // stamp itself differed -- and a no-op then produced a write, a map
    // refresh, an index bump and a commit record announcing a change that
    // never happened. That is the phantom-commit bug this function was
    // originally built to prevent (286 of them in one normalise pass). The
    // order matters: compare content, then stamp only if there is something to
    // stamp for.
    //
    // Stamp the moment before rendering, and only here.
    //
    // Every write path in the tool funnels through this function, so this is
    // the one place a record's "last touched" can be maintained without any
    // command having to remember to do it. Reading it back out of the file
    // instead (mtime) was the earlier design and it fails exactly when it
    // matters: copying the library to another machine, restoring a backup, or
    // unpacking an archive rewrites every mtime at once, so every record loses
    // its age and a listing ordered by recency becomes meaningless noise.
    // A field inside the file travels with the file.
    //
    // Set before the dry-run branch so `--dry-run` reports the same bytes it
    // would write, and before the `unchanged` comparison so that a real edit
    // always registers as a change (a stamp that moved *is* a change).
    // Splice from the original bytes, not a re-render: a change to one field
    // must not reformat the rest of the file. Falls back to a full render when
    // the layout cannot be matched.
    let probe = rec.render_for_write();
    if probe == rec.raw {
        // Nothing to do. Verified before any side effect, so a no-op cannot
        // touch the record, the map, the index or the change log.
        if a.has("dry-run") {
            print!("{}", render_dry_run(rec, op, &rel(root, &rec.path), a.has("verbose")));
        }
        return Ok(false);
    }
    rec.set("UPDATED", &now_iso_local());
    let body = rec.render_for_write();
    if std::env::var_os("ABOOK_SPLICE_DEBUG").is_some() {
        eprintln!(
            "[save] base={} 原字节={} 待写字节={} 相同={}",
            rec.base.is_some(),
            rec.raw.len(),
            body.len(),
            body == rec.raw
        );
    }
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
    // Version gate, immediately before the write.
    //
    // `--if-version` is the fingerprint the caller was shown when it *read* this
    // record. Checking it here is what makes the read-think-write gap safe: an
    // agent reads a record, spends minutes reasoning, and only then writes. Any
    // check that re-reads the file at command start passes trivially, because it
    // sees the current bytes -- the divergence happened before the command began.
    // Only a fingerprint carried across that gap can catch it.
    //
    // Deliberately after the no-op check: an edit that changes nothing cannot
    // overwrite anything, so it needs no version.
    let unchanged = body == rec.raw;
    if a.has("dry-run") {
        print!("{}", render_dry_run(rec, op, &rel(root, &rec.path), a.has("verbose")));
        return Ok(!unchanged);
    }
    if unchanged {
        return Ok(false);
    }
    if let Some(want) = a.get("if-version") {
        require_version(&rec.path, want)?;
    }
    write_atomic(&rec.path, &body)?;
    rec.rebase(&body);
    let back = Record::parse(&rec.path, rec.lib)?;
    // Compare ignoring trailing whitespace.
    //
    // The body is stored between tags, so the writer appends the newline that
    // separates it from `[/MEMORY_BODY]` while the in-memory body has none. An
    // exact comparison therefore failed on *every* body edit whose last line did
    // not already end in a blank -- and it failed *after* the write, so `split`
    // reported `INTEGRITY: 写入复读不一致` while the file on disk had already been
    // rewritten. A command that reports failure after changing the data is worse
    // than one that fails cleanly: the caller believes nothing happened, then
    // either retries (and gets "separator not found", because it was consumed) or
    // gives up while the database is half-transformed.
    if back.body.trim_end() != rec.body.trim_end() && !rec.body.trim().is_empty() {
        return integrity(format!(
            "写入已完成但复读不一致: {}（文件已改，请复核后再决定是否重做）",
            rel(root, &rec.path)
        ));
    }
    // atlas + index + counts
    crate::cmd_write_sync::sync_record(root, &back)?;
    {
        // `op` arrives already spelled as the library spells it
        // (`set-field`, `append-body`, ...), so it doubles as the slug and as
        // the KIND. The operation verb is derived from it by prefix.
        // Every edit command mutates an existing record in place, which the
        // library spells `PATCH`; the finer verb already lives in KIND.
        let operation = "PATCH";
        crate::cmd_admin::write_commit(
            root,
            &crate::cmd_admin::CommitInfo {
                slug: op,
            scope: "仅改动 1 条记录的目标字段，其余字节不变",
                operation,
                library: if rec.lib == crate::rec::Lib::Canonical {
                    "CANONICAL"
                } else {
                    "CANDIDATE"
                },
                summary: &format!("{} {}", op, back.title()),
                target: &rel(root, &rec.path),
                body: &format!("{op}

记录: {}
标题: {}", rel(root, &rec.path), back.title()),
            },
        )?;
    }
    Ok(true)
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
    // `set` is the generic escape hatch, so it has to know the few fields whose
    // change is not a plain text edit. Letting it write them created states no
    // other command can produce or repair:
    //
    //   set R ID <随便写>      -> ID 与文件名分叉，所有指向它的引用一起断
    //   set R FORMAT BOGUS     -> 写入一个解析器不认识的格式
    //   set R STATUS DELETED   -> 记录留在图里，目录数与地图数分叉
    //
    // 状态迁移有完整的同步路径（delete / restore），身份字段有它自己的规则，
    // 所以这三类不走 `set`。
    match canon.as_str() {
        "ID" => {
            return usage(
                "ID 是身份字段，不能用 set 改。\
它同时是文件名、其它记录 READ_WHEN 的指向目标和地图条目的键，改它会让这些引用一起失效。\
要改身份请用 abook rename（连文件名一起对齐）。",
            )
        }
        "STATUS" => {
            return usage(
                "STATUS 请用 delete / restore 改。\
     它们还会同步地图、目录计数和路由索引；直接 set 会让记录留在图里但目录数变少，\
     也就是 state 报「一致性 发现 1 个问题」的那种状态。",
            )
        }
        "FORMAT" => {
            let ok = value == "CANONICAL_MEMORY_V2"
                || value == "CANDIDATE_MEMORY_V2"
                || value == "AGENTSBOOK_V2_EXPERIMENT";
            if !ok {
                return usage(format!(
                    "FORMAT 只接受 CANONICAL_MEMORY_V2 / CANDIDATE_MEMORY_V2 / AGENTSBOOK_V2_EXPERIMENT，收到 {value}"
                ));
            }
        }
        _ => {}
    }
    rec.set(&canon, &value);
    let wrote = save(&root, &mut rec, a, "set-field")?;
    if !a.has("dry-run") {
        if wrote {
            crate::say_line!("{}= {}", canon, value);
        } else {
            crate::say_line!("{}= {} （与现值相同，未写入）", canon, value);
        }
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
    let had = rec.fields.iter().any(|(k, _)| k == &canon);
    rec.fields.retain(|(k, _)| k != &canon);
    let changed = save(&root, &mut rec, a, "unset-field")?;
    // Only claim to have removed something when the removal actually happened.
    // A dry run used to print "已移除 X" and write nothing; a field that was
    // never there printed the same line. Both make the caller believe the file
    // changed when it did not.
    if a.has("dry-run") {
        crate::say_line!(
            "[dry-run] {} {}",
            if had { "将移除" } else { "无此字段" },
            canon
        );
    } else if changed {
        crate::say_line!("已移除 {canon}");
    } else {
        crate::say_line!("没有 {} 这个字段，未写入", canon);
    }
    Ok(0)
}

fn read_payload(a: &Args) -> R<String> {
    if let Some(f) = a.get("file") {
        if f == "-" {
            return read_stdin_all();
        }
        return read_str(&expand_tilde(f));
    }
    // `-` means stdin, the way every other unix tool spells it.
    //
    // Without this, `abook update R --body -` wrote the single character `-` as
    // the whole body and returned 0: a heredoc is the normal way for an agent to
    // hand over a long text, so this quietly emptied records.
    if let Some(t) = a.get("text") {
        if t == "-" {
            return read_stdin_all();
        }
        return Ok(t.to_string());
    }
    if let Some(b) = a.get("body") {
        if b == "-" {
            return read_stdin_all();
        }
        return Ok(b.to_string());
    }
    read_stdin_all()
}

pub fn read_stdin_all() -> R<String> {
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
        crate::say_line!("正文已追加，现 {} 字节", rec.body.len());
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
        crate::say_line!("正文已前置，现 {} 字节", rec.body.len());
    }
    Ok(0)
}

pub fn cmd_patch(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let (path, lib) = resolve_record(&root, sel)?;
    guard(a, lib, "替换")?;
    let old = a.get("old").ok_or_else(|| EE::Usage("缺 --old".into()))?.to_string();
    // An empty needle is not "replace nothing", it is "match at every position".
    // `--old "$VAR"` with an unset VAR reached this line and, with `--all`, spliced
    // the replacement between every character of the body -- the record was
    // destroyed while the command reported success. Refusing is the only safe
    // reading of an empty needle.
    if old.is_empty() {
        return usage("--old 不能为空（空串会在每个字符之间插入替换内容，把正文毁掉）");
    }
    let new = a.get("new").unwrap_or("").to_string();
    let mut rec = Record::parse(&path, lib)?;
    // `--field F` means "the field named F". Reading it only when `--in field`
    // was also passed meant the documented form `patch X --field TITLE ...`
    // silently edited the body: the caller believes they are renaming a field
    // and the prose changes instead. There is no visible failure to catch it,
    // because a body match on `aa` usually succeeds.
    //
    // So the presence of `--field` decides, and `--in field` stays supported for
    // callers that already pass it. `--in body` still wins when given explicitly
    // with a field, because that combination is at least unambiguous about what
    // was asked for.
    let scope = a.get("in").unwrap_or("").to_string();
    let hay = if scope == "body" {
        "body".to_string()
    } else if !scope.is_empty() || a.get("field").is_some() {
        let f = a.get("field").unwrap_or("").to_string();
        if f.is_empty() {
            return usage("--in field 需要同时给出 --field <字段名>");
        }
        f
    } else {
        "body".to_string()
    };
    let hits: usize;
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
        if hits > 1 && !a.has("all") {
            return usage(format!("命中 {hits} 处，确认全部替换请加 --all"));
        }
        rec.set(&hay, &cur.replace(&old, &new));
    }
    save(&root, &mut rec, a, "patch-text")?;
    if !a.has("dry-run") {
        crate::say_line!("已替换 {hits} 处");
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
        crate::say_line!("标题已改为 {newname}");
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
        crate::say_line!("[dry-run] {} → {}", rel(&root, &path), rel(&root, &dst));
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
        &crate::cmd_admin::CommitInfo {
            slug: "rename-record",
            scope: "1 条记录改名，索引与地图指针同步改写",
            operation: "REPLACE",
            library: if lib == crate::rec::Lib::Canonical { "CANONICAL" } else { "CANDIDATE" },
            summary: &format!("改名 {old_rel} → {}", rel(&root, &dst)),
            target: &rel(&root, &dst),
            body: &format!("改名\n\n旧: {old_rel}\n新: {}", rel(&root, &dst)),
        },
    )?;
    crate::say_line!("{}", rel(&root, &dst));
    Ok(0)
}

pub fn cmd_move(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let branch = a.need_pos(1, "目标分支")?;
    let (path, lib) = resolve_record(&root, sel)?;
    guard(a, lib, "换分支")?;
    // Refuse a branch name that does not exist yet.
    //
    // `move` used to create whatever branch it was told to, so a typo did not
    // fail -- it silently fragmented the map into a new one-record branch, and
    // the record's real branch kept its old count. Creating a branch is a
    // decision, so it gets its own command (`branch add`) rather than happening
    // as a side effect of a misspelled move.
    let apath = root.join(lib.atlas());
    if apath.exists() {
        let at = crate::atlas::Atlas::load(&apath)?;
        if at.branch(branch).is_none() {
            let names: Vec<String> = at.branches.iter().map(|b| b.name()).collect();
            return notfound(format!(
                "分支「{branch}」不存在。现有分支：{}\n新建分支请用 abook branch add \"{branch}\"",
                names.join("、")
            ));
        }
    }
    let target = branch.to_string();
    let branch = target.as_str();
    if a.has("dry-run") {
        crate::say_line!("[dry-run] 把 {} 移到分支 {branch}", rel(&root, &path));
        return Ok(0);
    }
    crate::cmd_write_sync::move_branch(&root, lib, &rel(&root, &path), &branch)?;
    crate::cmd_admin::write_commit(
        &root,
        &crate::cmd_admin::CommitInfo {
            slug: "move-branch",
            scope: "1 条记录换分支，地图两分支计数同步",
            operation: "PATCH",
            library: if lib == crate::rec::Lib::Canonical { "CANONICAL" } else { "CANDIDATE" },
            summary: &format!("换分支 {} → {branch}", rel(&root, &path)),
            target: &rel(&root, &path),
            body: &format!("换分支\n\n记录: {}\n新分支: {branch}", rel(&root, &path)),
        },
    )?;
    crate::say_line!("已移到分支 {branch}");
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
    // 预览不建目标库目录（同 write/promote）：后面 561 行会走 save 的
    // dry-run 分支返回，真正写入时才需要目录存在。
    if !a.has("dry-run") {
        ensure_dir(&dir)?;
    }
    let dst = dir.join(format!("{id}.md"));
    rec.path = dst.clone();
    rec.lib = target_lib;
    save(&root, &mut rec, a, "copy-record")?;
    if !a.has("dry-run") {
        crate::say_line!("{}", rel(&root, &dst));
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
    if !a.has("dry-run") {
        ensure_dir(&dir)?;
    }
    let dst = dir.join(format!("{id}.md"));
    rec.path = dst.clone();
    rec.lib = Lib::Candidate;
    save(&root, &mut rec, a, "demote-record")?;
    // remove the canonical copy
    if !a.has("dry-run") {
        let old_rel = rel(&root, &path);
        std::fs::remove_file(&path)?;
        crate::cmd_write_sync::forget(&root, Lib::Canonical, &old_rel)?;
        crate::say_line!("{}", rel(&root, &dst));
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
        attach: None,
        raw: String::new(),
        devs: Vec::new(),
        header_inferred: false,
        // A brand-new record has no bytes on disk yet, so there is nothing to
        // splice: this one is written by rendering it in full.
        mtime: crate::util::now_secs(),
        base: None,
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
    if !a.has("dry-run") {
        ensure_dir(&root.join(lib.dir()))?;
    }
    save(&root, &mut out, a, "merge-records")?;
    if !a.has("dry-run") {
        crate::say_line!("{}", rel(&root, &out.path));
        if a.has("remove-sources") {
            for (p, l, _) in &recs {
                let r = rel(&root, p);
                let mut rr = Record::parse(p, *l)?;
                rr.set("STATUS", "SUPERSEDED");
                rr.set("SUPERSEDES", &id);
                write_atomic(p, &rr.render())?;
                crate::cmd_write_sync::forget(&root, *l, &r)?;
            }
            crate::say_line!("源记录已标记 SUPERSEDED");
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
        crate::say_line!("[dry-run] 拆成两段: {} 行 / {} 行", pre.len(), post.len());
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
    crate::say_line!("{}", rel(&root, &r2.path));
    Ok(0)
}
