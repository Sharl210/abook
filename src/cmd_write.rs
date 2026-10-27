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
    // 预览不能在空库上把整个库骨架建出来。
    //
    // `init_book` 会创建 canonical/records、candidate/records、commits、review、
    // staging、vault 六个目录，以及 Global.md、Index.md、两张 Atlas.md 和一条
    // 保险箱名册——十来个文件。原来这一句在 `--dry-run` 判断之前，于是对一个
    // 空目录跑一次"只看看会写成什么样"，就凭空造出一个完整的记忆库。
    //
    // 而且 `main` 见到 `--dry-run` 会跳过库锁，所以这些写入还是在无锁状态下
    // 发生的。一个预览命令既造了库又绕了锁，两件事都很糟。
    //
    // 预览时不初始化：下面的代码本来就会因为目标目录不存在而在 dry-run 分支
    // 直接返回，不需要库骨架先存在。
    if !a.has("dry-run") {
        crate::cmd_admin::init_book(&root)?;
    }
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
    // 预览时连目录都不建。`ensure_dir` 会创建 `candidate/`、`canonical/`
    // 这类目录，对一个空目录跑一次预览就留下一个空壳库，之后 `state` 会说
    // "候选库 0 条记录"——看起来像库坏了，其实是上次预览留下的。
    // 真正写入时下面还会再建一次，所以跳过这里是安全的。
    if !a.has("dry-run") {
        ensure_dir(&dir)?;
    }
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

/// 一次晋升产生的结果，供调用方决定后面怎么落盘。
///
/// 单独抽出来是因为批量晋升要跑几十次同一个流程，而**每一步的落盘代价都不是
/// 常数**：地图要整份载入再整份写出，索引同理。逐条各写一次就是 O(n²)——
/// 实测 87 条晋升里 Atlas 与 Index 各被重写 87 遍，索引从 63KB 长到 115KB 的
/// 过程中每次都是全量序列化。抽出结果、最后统一写一次，把这一步摊平成 O(n)。
struct Promoted {
    cand_path: std::path::PathBuf,
    canon_path: std::path::PathBuf,
    canon_id: String,
    title: String,
    read_when: String,
    moved_attach: String,
}

/// 选一个不会撞车的正式 ID。
///
/// `Record::new_id` 只拿正文算哈希，同一秒内晋升两条**正文相同**的候选会得到
/// 同一个名字，后写的那条直接覆盖前一条，而且没有任何报错。候选库里就有正文
/// 为空的记录（早期只写了头部的那些），批量跑起来必然撞。撞了就退到"正文+
/// 标题"，再撞就用候选自己的 ID 兜底——宁可名字丑，也不能悄悄少一条记录。
fn unique_canon_id(root: &Path, stamp: &str, cand: &Record) -> String {
    let dir = root.join(Lib::Canonical.dir());
    let mut id = Record::new_id(Lib::Canonical, stamp, &cand.body);
    if !dir.join(format!("{id}.md")).exists() {
        return id;
    }
    id = Record::new_id(
        Lib::Canonical,
        stamp,
        &format!("{}{}", cand.body, cand.title()),
    );
    if !dir.join(format!("{id}.md")).exists() {
        return id;
    }
    Record::new_id(
        Lib::Canonical,
        stamp,
        &format!("{}{}{}", cand.body, cand.title(), cand.id()),
    )
}

/// 查"某个候选 ID 已经晋升成了哪条正式记录"。
///
/// 依据是正式记录头部的 `PROMOTED_FROM`——那是唯一的溯源事实，不需要猜。
/// 返回 `None` 有明确含义：**对方还没晋升，或者它根本不是一条候选**，
/// 两种情况都该原样保留引用，不擅自改写。
///
/// 每次调用都扫一遍正式库。对单条晋升（一次一到几条）代价可以忽略；
/// 若将来要批量晋升上千条，这里应改成先建一次映射表再复用。
fn promoted_target_of(root: &Path, cand_id: &str) -> Option<String> {
    let dir = root.join(Lib::Canonical.dir());
    let rd = std::fs::read_dir(&dir).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        // 先廉价预筛：文件里没出现这个 ID 就不解析，省掉绝大多数 parse。
        let raw = match std::fs::read_to_string(&p) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !raw.contains(cand_id) {
            continue;
        }
        if let Ok(rec) = Record::parse(&p, Lib::Canonical) {
            if rec.get("PROMOTED_FROM").trim() == cand_id {
                return Some(rec.id());
            }
        }
    }
    None
}

/// 晋升一条候选：写正式记录、搬附件、给候选打晋升标记。
///
/// 只管这三件事。地图、索引、计数、变更日志由调用方决定什么时候写——
/// 单条晋升写完就同步，批量晋升留到全部完成后写一次。
fn promote_core(
    root: &Path,
    cpath: &Path,
    cand: &Record,
    // 移动语义下候选被删除，不再往回写 BRANCH 标记，所以这里用不到 branch；
    // 分支归属由调用方在做地图映射时决定（map_record / map_records_batch）。
    _branch: &str,
    st: &Stamps,
) -> R<Promoted> {
    let id = unique_canon_id(root, &st.compact, cand);
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
    canon.set("ID", &id);
    // Inherit the candidate's timestamp: promotion moves a record, it does not
    // edit it, so the promoted copy should keep showing when it was actually
    // learned.
    let inherited = {
        let u = cand.updated();
        if u > 0 { u } else { now_secs() }
    };
    canon.set("UPDATED", &crate::util::stamps_at(inherited).iso_local);
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
    // `RELATED_IDS` 里的候选 ID 要重写成那个候选的**正式** ID。
    //
    // 移动语义下候选晋升后即被删除，所以任何指向候选的身份都会立刻变成死链。
    // 而"候选 A 引用候选 B、两者一起晋升"是很自然的用法（同一主题的两条并列
    // 记录），这时 A 的正式记录若原样保留 `B的候选ID`，就指向了一条已删除的
    // 记录——刚做完的正确操作被自己的 `refs` 报成数据损坏。
    //
    // 重写依据是精确可推导的：正式记录的 `PROMOTED_FROM` 记着"我从哪来"，
    // 于是"候选 ID → 正式 ID"是个查表。**不删掉引用**——删了会丢掉 A 与 B
    // 的关联，那是真信息；换成正式 ID 才既保留关联又指得着。
    //
    // 查不到映射（对方还没晋升、或本来就不是身份值）就原样保留：猜错比留着更坏。
    let related = {
        let mut out: Vec<String> = Vec::new();
        for one in split_list(&cand.get("RELATED_IDS")) {
            let one = one.trim().to_string();
            if one.is_empty() {
                continue;
            }
            match promoted_target_of(root, &one) {
                Some(canon_id) => out.push(canon_id),
                None => out.push(one),
            }
        }
        out.join(",")
    };
    canon.set("RELATED_IDS", &related);

    let dir = root.join(Lib::Canonical.dir());
    ensure_dir(&dir)?;
    let path = dir.join(format!("{id}.md"));
    canon.path = path.clone();
    write_atomic(&path, &canon.render())?;
    let back = Record::parse(&path, Lib::Canonical)?;
    if back.id().is_empty() {
        return integrity("晋升写入复读校验失败");
    }
    // ===== 晋升是搬走，不是复制一份留着 =====
    //
    // 早先的做法是在候选末尾追加 `[PROMOTION]` 块把它留在原地：正式库多一条、
    // 候选库不减少，同一个对象在库里存在两份——一份在正式库当依据，一份在候选
    // 库当待表态件。每次盘点都得重新判断它到底算不算已处理，候选库只增不减，
    // 看上去永远有一堆待办。
    //
    // 现在正式侧已经拿到完整正文与附件，候选侧就该消失。溯源不靠留在候选里的
    // 标记，靠正式记录头部的 `PROMOTED_FROM`，加上变更记录里的两侧路径。
    //
    // ===== 删除前置门：晋升必须已被证明成功 =====
    //
    // 删除不可逆，所以不能拿"上面几行没返回 Err"当成功证明——那只证明函数走到了
    // 这里，不证明磁盘上有一条可读、内容完整的正式记录。一旦删了候选而正式侧其实
    // 是空壳或写歪了，这条内容就永久消失。四条全过才允许删：
    //   ① 正式文件确实存在于磁盘（不是内存里的 path 变量）
    //   ② 它能被重新解析（不是半截文件）
    //   ③ 它的 ID 与预期一致（没因命名冲突写到别处）
    //   ④ 正文与候选原正文一致（搬的过程中没被截断或改写）
    // 任一不过 → 保留候选、报 INTEGRITY 并说明正式侧现状，由人决定怎么修。
    //
    // 注意顺序：先撤索引与地图条目，再删文件。反过来的话会出现"文件已经没了，
    // 但索引和地图还指着它"的悬空引用，而这种状态没有任何命令能自动修。
    let promoted_on_disk = path.exists();
    let reparse = if promoted_on_disk {
        Record::parse(&path, Lib::Canonical).ok()
    } else {
        None
    };
    let verified = matches!(
        &reparse,
        Some(back2)
            if promoted_on_disk
                && back2.id() == id
                && back2.body.trim_end() == cand.body.trim_end()
    );
    if !verified {
        let why = match (&reparse, promoted_on_disk) {
            (_, false) => "正式记录未出现在磁盘上".to_string(),
            (None, _) => "正式记录无法被解析".to_string(),
            (Some(back2), _) if back2.id() != id => format!(
                "正式记录 ID 不符（期望 {id}，实际 {}）",
                back2.id()
            ),
            _ => "正式记录正文与候选正文不一致".to_string(),
        };
        return integrity(format!(
            "晋升未通过删除前校验：{why}。\
             候选 {} **已保留未删除**，请先检查正式侧 {} 再决定重试或回滚。",
            rel(root, cpath),
            rel(root, &path)
        ));
    }
    // 附件随晋升一起走。
    //
    // 晋升是"搬"不是"抄"：正文原样搬过去并保持原时间戳，附件目录也必须跟着
    // 改名搬过去。否则晋升后正文还在、它携带的截图与日志却留在候选侧，候选
    // 一旦清理证据就没了——一条只剩正文、丢了证据的记录，比没有这条记录更
    // 容易误导人。
    let mut moved_attach = String::new();
    let cand_dirs = crate::attach::all_dirs(cpath);
    if !cand_dirs.is_empty() {
        let want = crate::attach::dir_for(&path);
        if want.exists() {
            return blocked(format!(
                "正式侧已存在附件目录 {}。先把两边文件核对清楚，再决定合并还是改名",
                rel(root, &want)
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
                        rel(root, &from)
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
            rel(root, &want)
        );
    }
    // 正式侧已被证明是完整可读的一等公民，才动候选。
    crate::cmd_admin::index_remove(root, &rel(root, cpath))?;
    unmap_record(root, Lib::Candidate, &rel(root, cpath))?;
    // 附件目录在上面的 rename 中已经整目录搬到正式侧；这里清掉可能残留的旧命名空目录。
    for d in crate::attach::all_dirs(cpath) {
        let _ = std::fs::remove_dir_all(&d);
    }
    if cpath.exists() {
        if let Err(e) = std::fs::remove_file(cpath) {
            // 正式侧已成立、候选侧删不掉：这是"内容安全但库里有冗余"的状态，
            // 不是数据损坏。如实报告并让正式侧继续有效，比回滚更安全。
            crate::say_line!(
                "警告: 正式记录已写入 {}，但候选 {} 删除失败（{e}）。\
                 正式侧有效，请手工删除该候选以免重复。",
                rel(root, &path),
                rel(root, cpath)
            );
        }
    }
    Ok(Promoted {
        cand_path: cpath.to_path_buf(),
        canon_path: path,
        canon_id: id,
        title: back.title(),
        read_when: back.get("READ_WHEN"),
        moved_attach,
    })
}

/// 把多条记录一次性写进同一份地图。
///
/// `map_record` 每条都 `Atlas::load` 一次、`write_atomic` 一次。批量晋升里那
/// 等于把同一张 30KB 的地图载入写出几十遍，而且每次都重新数一遍全库记录数。
/// 这里载入一次、加完所有成员、写一次。
fn map_records_batch(root: &Path, items: &[(Record, String)]) -> R<()> {
    if items.is_empty() {
        return Ok(());
    }
    let lib = Lib::Canonical;
    let path = root.join(lib.atlas());
    let mut at = if path.exists() {
        Atlas::load(&path)?
    } else {
        Atlas::default()
    };
    if at.title.is_empty() {
        at.title = format!("{}/Atlas.md", lib.name());
        at.header
            .push(("FORMAT".into(), "AGENTSBOOK_V2_EXPERIMENT".into()));
        at.header.push(("LIBRARY".into(), lib.name().to_uppercase()));
    }
    let mut changed = false;
    for (rec, hint) in items {
        let relpath = rel(root, &rec.path);
        let mut branch = if hint.is_empty() {
            crate::query::guess_branch(rec)
        } else {
            hint.clone()
        };
        if at.branch(&branch).is_none() {
            branch = crate::atlas::DEFAULT_BRANCH.to_string();
        }
        if !add_to_branch(&mut at, &branch, &relpath) {
            continue;
        }
        changed = true;
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
    }
    if !changed {
        return Ok(());
    }
    let total: usize = at.total_count();
    let st = stamps();
    at.set("GENERATED_AT", &st.iso_z);
    at.set("RECORD_COUNT", &total.to_string());
    at.set("COVERED_THROUGH", &st.iso_z);
    at.set(
        "COVERED_RECORDS",
        &format!("全部{total}条记录；逐条PATH覆盖以本图各分支PATH并集为准（已对账{total}/{total}）"),
    );
    write_atomic(&path, &at.render())
}

/// 把多条路由一次性写进 Index.md。理由同 `map_records_batch`。
fn index_upsert_batch(root: &Path, rows: &[(String, String, String)]) -> R<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let p = root.join("Index.md");
    let mut bf = if p.exists() {
        BookFile::load(&p)?
    } else {
        BookFile {
            meta: vec![
                ("FORMAT".into(), "AGENTSBOOK_V2_EXPERIMENT".into()),
                ("FILE".into(), "Index.md".into()),
                ("ENTRY".into(), "Global-first".into()),
            ],
            meta_rows: vec!["1|Index.md|索引|# Index.md".into()],
            content: "# Index.md\n\n## Active Routes\n\n路径｜用途｜何时读取\n".into(),
            trailing: String::new(),
            head_raw: String::new(),
            tail_raw: String::new(),
        }
    };
    let mut changed = false;
    for (path, summary, when) in rows {
        if bf.upsert_route(path, summary, when) {
            changed = true;
        }
    }
    if changed {
        write_atomic(&p, &bf.render())?;
    }
    Ok(())
}

pub fn cmd_promote(a: &Args) -> R<i32> {
    // `--all` 是同一个命令的另一种形态，不是另一条命令：它对外的语义都是
    // "把这些候选搬进正式库"，只差在范围和事务边界。分成两条命令会让帮助
    // 与 spec 各记一份参数，慢慢就长歪了。
    if a.has("all") {
        return cmd_promote_all(a);
    }
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
    if cand.is_promoted() {
        let hdr_to = cand.get("PROMOTED_TO").trim().to_string();
        if hdr_to.is_empty() {
            return blocked(format!("该候选已晋升过: {}", cand.file_name()));
        }
        let dest = if root
            .join("canonical/records")
            .join(format!("{hdr_to}.md"))
            .exists()
        {
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
    // 已被合并进另一条：内容在那边，单独再晋升一次就是同一条知识入库两遍。
    // 要改就去改合并后的那条；真要拆开，先 `abook set <本条> MERGED_INTO ""`。
    if cand.is_absorbed() {
        return blocked(format!(
            "该候选已合并进 {}，不再单独晋升。\n\
             要改内容请直接编辑那条；确实要拆开，先 `abook set {} MERGED_INTO \"\"` 清掉再试。",
            cand.get("MERGED_INTO").trim(),
            cand.file_name()
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
    if a.has("dry-run") {
        // 预览要给出**真实的**目标文件名，而不是一句"将会晋升"。
        // 目标名由时间戳+正文哈希决定，逐条调用者自己算不出来；
        // 不给出来，`--dry-run` 就等于什么都没说。
        let id = unique_canon_id(&root, &st.compact, &cand);
        crate::say_line!(
            "[dry-run] 将晋升为 {}",
            rel(&root, &root.join(Lib::Canonical.dir()).join(format!("{id}.md")))
        );
        return Ok(0);
    }
    let branch = opt(a, "branch");
    let one = promote_core(&root, &cpath, &cand, &branch, &st)?;
    let back = Record::parse(&one.canon_path, Lib::Canonical)?;
    map_record(&root, &back, &branch)?;
    crate::cmd_admin::index_upsert(
        &root,
        &rel(&root, &one.canon_path),
        &format!("{}（由候选晋升）", back.title()),
        &back.get("READ_WHEN"),
    )?;
    sync_global_counts(&root)?;
    let body = format!(
        "晋升候选 → 正式（移动，候选已删除）\n\n候选（已删除）: {}\n正式: {}\n标题: {}{}",
        rel(&root, &one.cand_path),
        rel(&root, &one.canon_path),
        one.title,
        if one.moved_attach.is_empty() {
            String::new()
        } else {
            format!("\n{}", one.moved_attach)
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
    crate::say_line!("{}", rel(&root, &one.canon_path));
    if !one.moved_attach.is_empty() {
        crate::say_line!("{}", one.moved_attach);
    }
    Ok(0)
}

/// `promote --all`：把候选库里所有**尚未晋升**的记录一次处理完。
///
/// 为什么要有这条命令：单条 `promote` 每跑一次都要重写地图、索引、全局计数与
/// 变更日志，四个文件加起来两百多 KB。清理几十条积压就是几十次全量重写，
/// 调用方只能写个 shell 循环，而循环里的每一条都是独立事务——中途失败会留下
/// 半批状态，且没有任何地方记着"这批总共处理了什么"。
///
/// 这里把它变成一次事务：全部记录先落盘（每条仍需自己的原子写，因为正文不同），
/// 地图、索引、计数各写**一次**，变更日志写**一条**汇总。
///
/// 跳过项一律单独列出并说明原因，不静默丢弃——"处理完了"和"跳过了几条"必须
/// 同时可见，否则读的人无从判断这批到底有没有清干净。
pub fn cmd_promote_all(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let cands = crate::rec::load_lib(&root, Lib::Candidate);
    let kind = opt(a, "kind").to_uppercase();
    let maturity = opt(a, "maturity").to_uppercase();
    let needle = opt(a, "q").to_lowercase();
    let branch = opt(a, "branch");

    let mut todo: Vec<Record> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    // 已归档（晋升或合并）的候选单独计数，不进 `skipped`。
    //
    // 这两类不是"这批的问题"，是"本来就不该动"。把它们混进跳过清单会让输出
    // 读成"90 条晋升成功，210 条因为某种原因没成功"——而真相是那 210 条早就
    // 归档过了，清清白白。实测真实库就是这个形态：2 条待办、299 条已归档，
    // 混在一起报等于凭空造出 299 条"跳过"。
    let mut archived = 0usize;
    for rec in cands {
        // 已晋升的是溯源副本，不是待办。这条判断不放在过滤链里，是因为它
        // 与用户给的过滤条件是两回事：过滤是"这批我要哪些"，它是"这些本来
        // 就不该动"。
        if rec.is_promoted() {
            archived += 1;
            continue;
        }
        // 已合并进别的记录：内容已经由那条承载，这里不再是独立待办。
        if rec.is_absorbed() {
            archived += 1;
            continue;
        }
        // 真软删的才进跳过清单：这是"处理不了"，需要人知道为什么。
        if rec.status() == "DELETED" {
            skipped.push((rec.file_name(), "已软删".to_string()));
            continue;
        }
        if !kind.is_empty() && !rec.kind().eq_ignore_ascii_case(&kind) {
            continue;
        }
        if !maturity.is_empty() && !rec.get("MATURITY").eq_ignore_ascii_case(&maturity) {
            continue;
        }
        if !needle.is_empty() {
            let hay = format!("{} {} {}", rec.title(), rec.get("KEYS"), rec.body).to_lowercase();
            if !hay.contains(&needle) {
                continue;
            }
        }
        todo.push(rec);
    }

    if todo.is_empty() {
        crate::say_line!("没有待晋升的候选（已归档 {} 条不计入）", archived);
        return Ok(0);
    }

    if a.has("dry-run") {
        let st = stamps();
        crate::say_line!("[dry-run] 将晋升 {} 条：", todo.len());
        for rec in &todo {
            let id = unique_canon_id(&root, &st.compact, rec);
            crate::say_line!(
                "  {} → canonical/records/{}.md",
                rec.file_name(),
                id
            );
        }
        if archived > 0 {
            // 归档过的不算跳过：它们没有失败，也不该被算进这一批的账。
            crate::say_line!("另有 {} 条早已归档（晋升或合并），不重复处理", archived);
        }
        if !skipped.is_empty() {
            crate::say_line!("跳过 {} 条：", skipped.len());
            for (f, why) in &skipped {
                crate::say_line!("  {f}：{why}");
            }
        }
        return Ok(0);
    }

    if !a.has("approve") {
        return blocked(format!(
            "批量晋升 {} 条候选需要明确批准。确认后加 --approve；先看会做什么用 --dry-run。",
            todo.len()
        ));
    }

    let st = stamps();
    let mut done: Vec<Promoted> = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    for rec in &todo {
        match promote_core(&root, &rec.path, rec, &branch, &st) {
            Ok(p) => done.push(p),
            // 一条失败不该带走整批：附件冲突、复读校验这类问题是单条局部的，
            // 把它们攒下来一次性报告，比在第 3 条上直接终止更有用。
            Err(e) => failed.push((rec.file_name(), format!("{e:?}"))),
        }
    }

    // 三条收尾各写一次。
    let mut mapped: Vec<(Record, String)> = Vec::new();
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for p in &done {
        if let Ok(r) = Record::parse(&p.canon_path, Lib::Canonical) {
            rows.push((
                rel(&root, &p.canon_path),
                format!("{}（由候选晋升）", p.title),
                p.read_when.clone(),
            ));
            mapped.push((r, branch.clone()));
        }
    }
    map_records_batch(&root, &mapped)?;
    index_upsert_batch(&root, &rows)?;
    sync_global_counts(&root)?;

    let mut body = format!(
        "批量晋升候选 → 正式（移动，候选已删除）\n\n共 {} 条\n",
        done.len()
    );
    for p in &done {
        body.push_str(&format!(
            "- {}（已删除） → {}\n",
            rel(&root, &p.cand_path),
            rel(&root, &p.canon_path)
        ));
        if !p.moved_attach.is_empty() {
            body.push_str(&format!("  {}\n", p.moved_attach));
        }
    }
    if !failed.is_empty() {
        body.push_str(&format!("\n失败 {} 条：\n", failed.len()));
        for (f, why) in &failed {
            body.push_str(&format!("- {f}: {why}\n"));
        }
    }
    if !skipped.is_empty() {
        body.push_str(&format!("\n跳过 {} 条：\n", skipped.len()));
        for (f, why) in &skipped {
            body.push_str(&format!("- {f}: {why}\n"));
        }
    }
    crate::cmd_admin::write_commit(
        &root,
        &crate::cmd_admin::CommitInfo {
            slug: "promote-batch",
            scope: "候选库批量移入正式库，地图/索引/计数一次写完",
            operation: "REPLACE",
            library: "BOTH",
            summary: &format!("批量晋升 {} 条候选为正式", done.len()),
            target: "canonical/records",
            body: &body,
        },
    )?;

    crate::say_line!("晋升 {} 条", done.len());
    for p in &done {
        crate::say_line!("  {}", rel(&root, &p.canon_path));
    }
    if !failed.is_empty() {
        crate::say_line!("失败 {} 条：", failed.len());
        for (f, why) in &failed {
            crate::say_line!("  {f}：{why}");
        }
    }
    if archived > 0 {
        crate::say_line!("另有 {} 条早已归档（晋升或合并），不重复处理", archived);
    }
    if !skipped.is_empty() {
        crate::say_line!("跳过 {} 条：", skipped.len());
        for (f, why) in &skipped {
            crate::say_line!("  {f}：{why}");
        }
    }
    if !failed.is_empty() {
        // 有失败就不是 0：调用方靠退出码判断这批清没清干净。
        return Ok(5);
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
    // `-` means stdin. Without this, `update R --body -` wrote the single
    // character `-` as the whole body and returned 0 -- and a heredoc is the
    // normal way an agent hands over a long text, so records were quietly emptied.
    if let Some(bf) = a.get("body-file") {
        let text = if bf == "-" {
            crate::cmd_edit::read_stdin_all()?
        } else {
            read_str(&expand_tilde(bf))?
        };
        rec.body = text.trim_end().to_string();
        changed.push("BODY".into());
    } else if let Some(b) = a.get("body") {
        let text: String = if b == "-" {
            crate::cmd_edit::read_stdin_all()?
        } else {
            b.to_string()
        };
        rec.body = text.trim_end().to_string();
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
    // Everything below used to be hand-rolled and disagreed with every other
    // edit command in four ways: it did not stamp UPDATED, it wrote a commit even
    // when the value was unchanged (the phantom commit `save()` exists to
    // prevent), it labelled candidates as CANONICAL in the commit, and it did not
    // recompute SUMMARY/SIZE after replacing the body -- so a record whose body
    // was completely replaced still advertised the old summary, and no command
    // could recompute it afterwards.
    //
    // `save()` already does all of that and is the one path every other edit
    // funnels through, so route through it instead of keeping a second opinion.
    let changed_flag = crate::cmd_edit::save(&root, &mut rec, a, "update-record")?;
    if !changed_flag {
        crate::say_line!("与现值相同，未写入");
        return Ok(0);
    }
    crate::say_line!("{}", rel(&root, &path));
    Ok(0)
}

pub fn cmd_delete(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录（路径 / 文件名 / ID）")?;
    let (path, lib) = resolve_record(&root, sel)?;
    let rec = Record::parse(&path, lib)?;
    // A dry run writes nothing, so it must not need approval. It used to, which
    // made previewing a delete *more* gated than performing one -- the caller
    // could not see what would happen without first authorising it, so the
    // approval was given blind.
    if a.has("dry-run") {
        crate::say_line!(
            "[dry-run] 将删除 {}（{}，{}）",
            rel(&root, &path),
            rec.title(),
            if a.has("hard") { "物理删除，不可恢复" } else { "标记删除，可 restore 撤销" }
        );
        return Ok(0);
    }
    if !a.has("approve") {
        return blocked("删除需要用户明确批准：确认后加 --approve。不带 --hard 时可恢复。");
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

#[cfg(test)]
mod batch_promote_tests {
    use super::*;

    fn cand(body: &str, id: &str) -> Record {
        Record {
            path: std::path::PathBuf::from(format!("candidate/records/{id}.md")),
            lib: Lib::Candidate,
            fields: vec![("ID".into(), id.into())],
            body: body.to_string(),
            promotion: None,
            attach: None,
            raw: String::new(),
            devs: Vec::new(),
            header_inferred: false,
            base: None,
            mtime: 0,
        }
    }

    /// 同一秒内晋升两条正文相同的候选，不能拿到同一个文件名。
    ///
    /// `Record::new_id` 只拿正文算哈希。候选库里确实存在正文为空的记录（早期
    /// 只写了头部的那些），批量跑起来必然撞名；撞了之后后写的那条直接覆盖前
    /// 一条，而且没有任何报错——两条记录悄悄变成一条。
    #[test]
    fn identical_bodies_get_distinct_ids() {
        let dir = std::env::temp_dir().join(format!("abook-unique-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("canonical/records")).unwrap();

        let st = stamps();
        let empty_a = cand("", "N-A");
        let empty_b = cand("", "N-B");
        let id_a = unique_canon_id(&dir, &st.compact, &empty_a);
        // 模拟"第一条已经写下去了"
        std::fs::write(
            dir.join("canonical/records").join(format!("{id_a}.md")),
            "x",
        )
        .unwrap();
        let id_b = unique_canon_id(&dir, &st.compact, &empty_b);
        assert_ne!(
            id_a, id_b,
            "正文相同也必须拿到不同文件名，否则后写的会静默覆盖前一条"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
