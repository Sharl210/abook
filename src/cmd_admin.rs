//! Administrative commands: init, refresh atlases, commit records, sync index,
//! normalise legacy names.

use crate::atlas::{add_to_branch, Atlas, DEFAULT_BRANCH};
use crate::book::BookFile;
use crate::rec::{load_lib, Lib, Record};
use crate::util::*;
use std::path::{Path, PathBuf};

/// Rebuild branch paths from what is actually on disk.
/// Keeps existing KEYS/SUMMARY/READ_WHEN and per-branch REVISION bumps.
/// Returns the number of paths now mapped.
/// What a rebuild did, including what it removed.
///
/// The count alone was enough to add paths and useless for everything else.
/// A rebuild that drops a `PATH=` entry because it cannot find the file is
/// making a claim about the library -- "this record is gone" -- and when a
/// rename desynchronised 228 of them it made that claim 228 times without
/// saying so, emptying 11 branches while every caller saw only a number.
#[derive(Default)]
pub struct RefreshReport {
    pub mapped: usize,
    /// `(branch, entry)` for every path dropped because no file matched it.
    pub dropped: Vec<(String, String)>,
}

pub fn refresh_atlas(root: &Path, lib: Lib, dry: bool) -> R<RefreshReport> {
    let path = root.join(lib.atlas());
    let mut at = if path.exists() {
        Atlas::load(&path)?
    } else {
        let mut a = Atlas::default();
        a.title = format!("{}/Atlas.md", lib.name());
        a.header.push(("FORMAT".into(), "AGENTSBOOK_V2_EXPERIMENT".into()));
        a.header
            .push(("LIBRARY".into(), lib.name().to_uppercase()));
        a
    };
    let disk = list_md(&root.join(lib.dir()));
    let disk_rel: Vec<String> = disk.iter().map(|p| rel(root, p)).collect();

    // Snapshot each card's path set before touching it. A rebuild that maps
    // exactly the same records must not count as a revision: `add_to_branch`
    // bumps REVISION on every insertion, so rebuilding through it used to
    // inflate REVISION by the number of paths on every single run, making
    // `refresh` non-idempotent.
    let before: Vec<(String, Vec<String>)> = at
        .branches
        .iter()
        .map(|b| (b.name(), b.paths()))
        .collect();
    // Which branches each existing record was mapped from. A record may
    // legitimately appear in several branches -- the shipped library has 2
    // such records in canonical and 11 in candidate. Keeping only the first
    // match silently deleted the other memberships, so this keeps them all.
    let mut owner: Vec<(String, String)> = Vec::new();
    for b in &at.branches {
        for p in b.paths() {
            let base = p.rsplit('/').next().unwrap_or(&p).to_string();
            owner.push((base, b.name()));
        }
    }
    // Keep each card's existing path order and only drop entries whose file
    // is gone; brand-new records are appended at the end. Rebuilding in disk
    // order instead reshuffled every multi-record card on every run, which is
    // churn the map never asked for and buries the one line that did change.
    let disk_bases: Vec<String> = disk_rel
        .iter()
        .map(|d| d.rsplit('/').next().unwrap_or(d).to_string())
        .collect();
    let mut dropped: Vec<(String, String)> = Vec::new();
    for b in at.branches.iter_mut() {
        let orig = b.raw_paths();
        let mut kept: Vec<String> = Vec::with_capacity(orig.len());
        for p in &orig {
            let base = p.rsplit('/').next().unwrap_or(p.as_str()).to_string();
            if disk_bases.contains(&base) {
                kept.push(p.clone());
            } else {
                dropped.push((b.name(), p.clone()));
            }
        }
        // Only touch the line when an element really disappeared; an unchanged
        // card must stay byte-identical.
        if kept.len() != orig.len() {
            b.set_paths_raw(&kept);
        }
    }
    let mut mapped = 0usize;
    for d in &disk_rel {
        let base = d.rsplit('/').next().unwrap_or(d).to_string();
        let mut branches: Vec<String> = owner
            .iter()
            .filter(|(b, _)| b == &base)
            .map(|(_, br)| br.clone())
            .collect();
        if branches.is_empty() {
            let guess = if let Ok(rec) = Record::parse(&root.join(d), lib) {
                crate::query::guess_branch(&rec)
            } else {
                DEFAULT_BRANCH.to_string()
            };
            branches.push(guess);
        }
        branches.retain(|b| at.branch(b).is_some());
        if branches.is_empty() {
            branches.push(DEFAULT_BRANCH.to_string());
        }
        for branch in branches {
            if let Some(idx) = at.branches.iter().position(|b| b.name() == branch) {
                let mut ps = at.branches[idx].raw_paths();
                if !ps.iter().any(|p| p.rsplit('/').next().unwrap_or(p) == base) {
                    // Append, never insert: existing order carries meaning to
                    // whoever reads the map. Write the new element in whatever
                    // form this card already uses, so a card that lists bare
                    // names does not suddenly gain one full path.
                    let bare = ps.iter().filter(|p| !p.contains('/')).count();
                    let full = ps.len() - bare;
                    let elem = if bare > full { base.clone() } else { d.clone() };
                    ps.push(elem);
                    at.branches[idx].set_paths_raw(&ps);
                    mapped += 1;
                }
            } else if add_to_branch(&mut at, &branch, d) {
                mapped += 1;
            }
        }
    }
    // recompute COUNT / MAXSIZE / REVISION
    for b in at.branches.iter_mut() {
        let ps = b.paths();
        let n = ps.len();
        let mut maxsz = 0u64;
        for p in &ps {
            let full = crate::atlas::resolve_path(&root, lib, p);
            if let Ok(m) = std::fs::metadata(&full) {
                if m.len() > maxsz {
                    maxsz = m.len();
                }
            }
        }
        if n == 0 {
            // An empty card keeps whatever it had; writing COUNT=0 would
            // claim the branch exists but maps nothing, which is not a state
            // the library uses.
            continue;
        }
        b.set("COUNT", &n.to_string());
        b.set("MAXSIZE", &maxsz.to_string());
    }
    // Only a real change to a card's path set counts as a revision.
    for b in at.branches.iter_mut() {
        let name = b.name();
        let now_paths: Vec<String> = b
            .paths()
            .iter()
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
            .collect();
        if let Some((_, old)) = before.iter().find(|(n, _)| *n == name) {
            let old_set: Vec<String> = old
                .iter()
                .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
                .collect();
            if old_set == now_paths {
                continue;
            }
        }
        let rev = b.get("REVISION").parse::<u64>().unwrap_or(0) + 1;
        b.set("REVISION", &rev.to_string());
    }
    at.branches.retain(|b| b.count() > 0 || b.get("STATUS") == "ACTIVE");

    let now = stamps();
    let total: usize = at.total_count();
    at.set("GENERATED_AT", &now.iso_z);
    at.set("RECORD_COUNT", &total.to_string());
    at.set("COVERED_THROUGH", &now.iso_z);
    at.set(
        "COVERED_RECORDS",
        &format!("全部{total}条记录；逐条PATH覆盖以本图各分支PATH并集为准（已对账{total}/{total}）"),
    );

    if !dry {
        write_atomic(&path, &at.render())?;
    }
    Ok(RefreshReport { mapped, dropped })
}

/// Everything a commit record needs in order to look like the library's own.
///
/// The first version of this writer invented its own three-field shape
/// (`ID/TIME/KIND`) and wrote bare `Z` timestamps. The live library uses none
/// of that: its 107 commits carry `FORMAT`, `COMMIT_ID` or `ID`, `OPERATION`,
/// `KIND`, `SCOPE`, `SUMMARY`, `TARGET`, `LIBRARY`, and spell time as local
/// wall clock with an explicit offset and zone name in 38 of 54 cases, never
/// as `Z`. A tool that writes a different dialect is not a proxy for this
/// library; it slowly forks it.
pub struct CommitInfo<'a> {
    /// Short machine name of the operation, e.g. `set-field`.
    pub slug: &'a str,
    /// `CREATE` / `PATCH` / `REPLACE` / `DELETE` / `RESTORE` / `PATCH_ATLAS`.
    pub operation: &'a str,
    /// Which library was touched: `CANONICAL` / `CANDIDATE` / `BOTH` / `NONE`.
    pub library: &'a str,
    /// One-line statement of what changed, for the index line of a listing.
    pub summary: &'a str,
    /// What was affected, relative to the root. Kept verbatim.
    pub target: &'a str,
    /// The blast radius in words: what else this write could have touched.
    /// Distinct from `target` (a path) and from `kind` (a verb), and the reason
    /// the block carries both.
    pub scope: &'a str,
    /// Narrative body placed below the record block.
    pub body: &'a str,
}

/// Reject a commit that would not parse back as the library's own format.
///
/// This runs before the file exists, so a formatting mistake surfaces as a
/// refused write instead of a commit nobody notices is malformed. The rules
/// are the ones the reader depends on: ASCII upper-case keys, no newline or
/// `;` inside a value, a time that round-trips, and a summary that is actually
/// filled in.
pub fn vet_commit_public(name: &str, block: &str) -> Result<(), String> { vet_commit(name, block) }

fn vet_commit(name: &str, block: &str) -> Result<(), String> {
    if !block.starts_with("[COMMIT]\n") {
        return Err("块首必须是 [COMMIT]".into());
    }
    if !block.trim_end().ends_with("[/COMMIT]") {
        return Err("块尾必须是 [/COMMIT]".into());
    }
    let inner = block
        .trim_start_matches("[COMMIT]\n")
        .trim_end()
        .trim_end_matches("[/COMMIT]");
    let mut seen: Vec<&str> = Vec::new();
    for line in inner.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .ok_or_else(|| format!("字段行缺少 '=': {line:?}"))?;
        if k.is_empty()
            || !k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(format!("键名必须是 ASCII 大写: {k:?}"));
        }
        if seen.contains(&k) {
            return Err(format!("键名重复: {k}"));
        }
        seen.push(k);
        if v.contains(';') {
            return Err(format!("值里不能含 ';'（会被读成下一个字段）: {k}"));
        }
        if v.trim().is_empty() {
            return Err(format!("字段 {k} 不能为空"));
        }
    }
    for must in ["FORMAT", "ID", "OPERATION", "TIME", "SCOPE", "SUMMARY"] {
        if !seen.contains(&must) {
            return Err(format!("缺少必需字段 {must}"));
        }
    }
    // The time must survive a round trip through the shared parser, otherwise
    // `history`/`recent` silently fall back to mtime for this file.
    let time = inner
        .lines()
        .find_map(|l| l.trim().strip_prefix("TIME="))
        .unwrap_or("");
    if crate::util::parse_stamp(time).is_none() {
        return Err(format!("TIME 无法解析: {time:?}"));
    }
    if !name.ends_with(".md") {
        return Err("文件名必须以 .md 结尾".into());
    }
    Ok(())
}

/// Append one commit record; returns the path written.
pub fn write_commit(root: &Path, info: &CommitInfo) -> R<PathBuf> {
    let dir = root.join("commits");
    ensure_dir(&dir)?;
    let st = stamps();
    let mut slug_clean: String = info
        .slug
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    while slug_clean.contains("--") {
        slug_clean = slug_clean.replace("--", "-");
    }
    let slug_clean = slug_clean.trim_matches('-').to_string();
    let slug_clean = if slug_clean.is_empty() { "commit".to_string() } else { slug_clean };
    let base = format!("{}-{}.md", st.compact, slug_clean);
    let mut p2 = dir.join(&base);
    let mut i = 1;
    while p2.exists() {
        p2 = dir.join(format!("{}-{}-{i}.md", st.compact, slug_clean));
        i += 1;
    }
    let name = p2
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let id = name.trim_end_matches(".md").to_string();

    // Field order follows the library's own records: identity, then what kind
    // of change this is, then when, then the scope, then the readable summary.
    let block = format!(
        "[COMMIT]\nFORMAT=AGENTSBOOK_V2_EXPERIMENT\nCOMMIT_ID={id}\nID={id}\n\
OPERATION={}\nKIND={}\nLIBRARY={}\nTIME={}\nTARGET={}\nSCOPE={}\nSUMMARY={}\n[/COMMIT]",
        info.operation,
        slug_clean,
        info.library,
        st.iso_local,
        info.target,
        info.scope,
        info.summary,
    );
    // Refused rather than written: a malformed commit is invisible in normal
    // use and only hurts later, so it stops here.
    vet_commit(&name, &block)
        .map_err(|e| crate::util::EE::Blocked(format!("变更记录自检未通过，拒绝写入: {e}")))?;
    let content = format!("{block}\n\n{}\n", info.body.trim_end());
    write_atomic(&p2, &content)?;
    let gp = root.join("Global.md");
    if gp.exists() {
        if let Ok(mut g) = BookFile::load(&gp) {
            g.set_state("LATEST_COMMIT", &name);
            write_atomic(&gp, &g.render())?;
        }
    }
    Ok(p2)
}

/// Ensure a library skeleton exists.
pub fn init_book(root: &Path) -> R<Vec<String>> {
    let mut made = Vec::new();
    for d in [
        "canonical/records",
        "candidate/records",
        "commits",
        "review",
        "staging",
        "vault",
    ] {
        let p = root.join(d);
        if !p.exists() {
            ensure_dir(&p)?;
            made.push(d.to_string());
        }
    }
    let g = root.join("Global.md");
    if !g.exists() {
        write_atomic(&g, &initial_global())?;
        made.push("Global.md".into());
    }
    let i = root.join("Index.md");
    if !i.exists() {
        write_atomic(&i, &initial_index())?;
        made.push("Index.md".into());
    }
    for lib in [Lib::Canonical, Lib::Candidate] {
        let p = root.join(lib.atlas());
        if !p.exists() {
            refresh_atlas(root, lib, false)?;
            made.push(lib.atlas().to_string());
        }
    }
    // The ledger is a normal vault entry, so a fresh vault has none yet.
    if crate::vault::read_entry(root, crate::vault::LEDGER_ID).is_err() {
        crate::vault::sync_ledger(root)?;
        made.push("vault/名册".into());
    }
    Ok(made)
}

fn initial_global() -> String {
    let st = stamps();
    format!(
        "[MEMORY_BOOK_V2]\nFORMAT=AGENTSBOOK_V2_EXPERIMENT\nFILE=Global.md\nENTRY=Global-first\nCONTENTS=\n\
1|Global.md|入口|# Global.md\n2|Current State|状态节|## Current State\n3|Entry Order|入口顺序|## Entry Order\n\
4|Credential Identity Index|凭据身份索引|## Credential Identity Index\n5|Map Index|地图索引|## Map Index\n\
6|Migration Record|迁移记录|## Migration Record\n[/MEMORY_BOOK_V2]\n\n[MEMORY_CONTENT]\n# Global.md\n\n\
## Current State\nMEMORY_BOOK_FORMAT=AGENTSBOOK_V2_EXPERIMENT\nMEMORY_BOOK_ROOT={root}\n\
CANONICAL_RECORDS=0\nCANDIDATE_RECORDS=0\nCANONICAL_ATLAS=canonical/Atlas.md\n\
CANDIDATE_ATLAS=candidate/Atlas.md\nCANONICAL_ATLAS_COVERED_THROUGH={now}\n\
CANDIDATE_ATLAS_COVERED_THROUGH={now}\nLATEST_COMMIT=\n\n## Entry Order\n\
1. 先读本节状态与地图索引，再读 Index.md、两张 Atlas 与命中记录。\n\
2. 写入一律先落候选，正式库只接受用户明确批准的内容。\n\
3. 凭据只进 vault，普通记录只保留引用。\n\n## Credential Identity Index\n\n\
本节为身份投影，不含凭据值。\n\n标识｜用途｜归属系统/来源状态｜允许用途｜取用条件｜值披露级别\n\n\
## Map Index\n- 正式库地图：`canonical/Atlas.md`\n- 候选库地图：`candidate/Atlas.md`\n\
- 记录目录：`canonical/records/`、`candidate/records/`\n- 晋升记录：`review/`\n- 变更记录：`commits/`\n\n\
## Migration Record\n- {now} 由 abook init 初始化\n\n[/MEMORY_CONTENT]\n",
        root = "{MEMORY_BOOK_ROOT}",
        now = st.iso_z
    )
}

fn initial_index() -> String {
    "[MEMORY_BOOK_V2]\nFORMAT=AGENTSBOOK_V2_EXPERIMENT\nFILE=Index.md\nENTRY=Global-first\nCONTENTS=\n\
1|Index.md|索引|# Index.md\n[/MEMORY_BOOK_V2]\n\n[MEMORY_CONTENT]\n# Index.md\n\n## Active Routes\n\n\
路径｜用途｜何时读取\n\n[/MEMORY_CONTENT]\n"
        .to_string()
}

/// Add or update one Index.md route row.
pub fn index_upsert(root: &Path, path: &str, summary: &str, when: &str) -> R<()> {
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
    let changed = bf.upsert_route(path, summary, when);
    if changed {
        write_atomic(&p, &bf.render())?;
    }
    Ok(())
}

pub fn index_remove(root: &Path, path: &str) -> R<bool> {
    let p = root.join("Index.md");
    if !p.exists() {
        return Ok(false);
    }
    let mut bf = BookFile::load(&p)?;
    let r = bf.remove_route(path);
    if r {
        write_atomic(&p, &bf.render())?;
    }
    Ok(r)
}

/// Rename legacy sequential records to the <prefix>-<stamp>-<hash8> scheme.
pub fn normalize_names(root: &Path, lib: Lib, dry: bool) -> R<Vec<(String, String)>> {
    let mut changes = Vec::new();
    for rec in load_lib(root, lib) {
        let name = rec.file_name();
        if Record::is_scheme_name(&name) {
            continue;
        }
        // derive stamp from the record's own ID if it already has one
        let id = rec.id();
        let stamp = if id.len() == 15 && id.as_bytes()[8] == b'T' {
            id.clone()
        } else if let Some(rest) = name.strip_prefix("AB2-") {
            let _ = rest;
            stamps().compact
        } else {
            stamps().compact
        };
        let new_stem = Record::new_id(lib, &stamp, &rec.body);
        let new_name = format!("{new_stem}.md");
        if !dry {
            let dst = root.join(lib.dir()).join(&new_name);
            std::fs::rename(&rec.path, &dst)?;
            // Keep header ID in sync with the filename -- and change nothing
            // else. This used to call `render()`, which re-emits the header in
            // canonical key order; on a record whose `ID=;PROJECT=;TASK=;...`
            // were packed onto one line that split them into four, added
            // `KEYS=`/`SIZE=`, and dropped fields `render()` does not know
            // about. Run over the shipped library it rewrote 228 records whose
            // only change was supposed to be the file name.
            if let Ok(mut r2) = Record::parse(&dst, lib) {
                if r2.id() != new_stem {
                    r2.set("ID", &new_stem);
                    let out = r2.render_for_write();
                    write_atomic(&dst, &out)?;
                }
            }
        }
        changes.push((name, new_name));
    }
    // A rename that leaves the map pointing at the old name is not a rename,
    // it is a desynchronisation. Measured on a copy of the real library:
    // renaming 228 records and then running `refresh` emptied the 记忆系统
    // branch from 15 records to 0, because rebuild could not match the stale
    // `PATH=AB2-C-001.md` entries against any file on disk and read them as
    // deletions.
    //
    // The map is edited here, in the same operation, so the two never diverge.
    // `refresh` is the command for "rebuild from disk"; a rename should not
    // require the user to know that it must run afterwards.
    if !dry && !changes.is_empty() {
        let path = root.join(lib.atlas());
        if path.exists() {
            let mut at = Atlas::load(&path)?;
            let mut touched = false;
            for b in at.branches.iter_mut() {
                let orig = b.raw_paths();
                let mut next: Vec<String> = Vec::with_capacity(orig.len());
                for p in &orig {
                    let base: &str = p.rsplit('/').next().unwrap_or(p.as_str());
                    match changes.iter().find(|(o, _)| o == base) {
                        Some((_, n)) => {
                            // Preserve any directory prefix the entry carried.
                            let rewritten = if p.ends_with(base) && p.len() > base.len() {
                                format!("{}{n}", &p[..p.len() - base.len()])
                            } else {
                                n.clone()
                            };
                            next.push(rewritten);
                            touched = true;
                        }
                        None => next.push(p.clone()),
                    }
                }
                if next != orig {
                    b.set_paths_raw(&next);
                }
            }
            if touched {
                write_atomic(&path, &at.render())?;
            }
        }
    }
    Ok(changes)
}
