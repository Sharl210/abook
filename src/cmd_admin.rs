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
pub fn refresh_atlas(root: &Path, lib: Lib, dry: bool) -> R<usize> {
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

    // remember which branch each existing path was in
    let mut owner: Vec<(String, String)> = Vec::new();
    for b in &at.branches {
        for p in b.paths() {
            let base = p.rsplit('/').next().unwrap_or(&p).to_string();
            owner.push((base, b.name()));
        }
    }
    // clear all paths, then re-add in disk order
    for b in at.branches.iter_mut() {
        b.set_paths(&[]);
        b.set("COUNT", "0");
    }
    let mut mapped = 0usize;
    for d in &disk_rel {
        let base = d.rsplit('/').next().unwrap_or(d).to_string();
        let mut branch = owner
            .iter()
            .find(|(b, _)| b == &base)
            .map(|(_, br)| br.clone())
            .unwrap_or_else(|| {
                if let Ok(rec) = Record::parse(&root.join(d), lib) {
                    crate::query::guess_branch(&rec)
                } else {
                    DEFAULT_BRANCH.to_string()
                }
            });
        if at.branch(&branch).is_none() {
            branch = DEFAULT_BRANCH.to_string();
        }
        if branch == DEFAULT_BRANCH && at.branch(DEFAULT_BRANCH).is_none() {
            // create it lazily below
        }
        if add_to_branch(&mut at, &branch, d) {
            mapped += 1;
        }
    }
    // recompute COUNT / MAXSIZE / REVISION
    for b in at.branches.iter_mut() {
        let ps = b.paths();
        let n = ps.len();
        let mut maxsz = 0u64;
        for p in &ps {
            if let Ok(m) = std::fs::metadata(root.join(p)) {
                if m.len() > maxsz {
                    maxsz = m.len();
                }
            }
        }
        if n == 0 {
            continue;
        }
        b.set("COUNT", &n.to_string());
        b.set("MAXSIZE", &maxsz.to_string());
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
    Ok(mapped)
}

/// Append one commit record; returns the path written.
pub fn write_commit(root: &Path, slug: &str, body: &str) -> R<PathBuf> {
    let dir = root.join("commits");
    ensure_dir(&dir)?;
    let st = stamps();
    let mut slug_clean: String = slug
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    while slug_clean.contains("--") {
        slug_clean = slug_clean.replace("--", "-");
    }
    let slug_clean = slug_clean.trim_matches('-').to_string();
    let name = format!("{}-{}.md", st.compact, slug_clean);
    let p = dir.join(&name);
    let mut i = 1;
    let mut p2 = p.clone();
    while p2.exists() {
        p2 = dir.join(format!("{}-{}-{}.md", st.compact, slug_clean, i));
        i += 1;
    }
    let content = format!(
        "[COMMIT]\nID={}\nTIME={}\nKIND={}\n[/COMMIT]\n\n{}",
        p2.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        st.iso_z,
        slug_clean,
        body.trim_end()
    );
    write_atomic(&p2, &content)?;
    // update LATEST_COMMIT in Global.md
    let gp = root.join("Global.md");
    if gp.exists() {
        if let Ok(mut g) = BookFile::load(&gp) {
            g.set_state(
                "LATEST_COMMIT",
                &p2.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            );
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
            path: p.to_string_lossy().to_string(),
            meta: vec![
                ("FORMAT".into(), "AGENTSBOOK_V2_EXPERIMENT".into()),
                ("FILE".into(), "Index.md".into()),
                ("ENTRY".into(), "Global-first".into()),
            ],
            meta_rows: vec!["1|Index.md|索引|# Index.md".into()],
            content: "# Index.md\n\n## Active Routes\n\n路径｜用途｜何时读取\n".into(),
            trailing: String::new(),
        }
    };
    bf.upsert_route(path, summary, when);
    write_atomic(&p, &bf.render())
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
            // keep header ID in sync with the filename
            if let Ok(mut r2) = Record::parse(&dst, lib) {
                if r2.id() != new_stem {
                    r2.set("ID", &new_stem);
                    write_atomic(&dst, &r2.render())?;
                }
            }
        }
        changes.push((name, new_name));
    }
    Ok(changes)
}
