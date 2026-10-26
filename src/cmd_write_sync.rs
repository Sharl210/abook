//! Shared sync helpers: keep atlas, index, Global counts and the commit log
//! consistent after any record mutation.

use crate::atlas::{add_to_branch, remove_from_all, Atlas, DEFAULT_BRANCH};
use crate::book::BookFile;
use crate::rec::{Lib, Record};
use crate::util::*;
use std::path::Path;

/// Register or refresh a record in its library atlas and the route index.
pub fn sync_record(root: &Path, rec: &Record) -> R<()> {
    let lib = rec.lib;
    let relpath = rel(root, &rec.path);
    let apath = root.join(lib.atlas());
    let mut at = if apath.exists() {
        Atlas::load(&apath)?
    } else {
        let mut a = Atlas::default();
        a.title = format!("{}/Atlas.md", lib.name());
        a.header.push(("FORMAT".into(), "AGENTSBOOK_V2_EXPERIMENT".into()));
        a.header.push(("LIBRARY".into(), lib.name().to_uppercase()));
        a
    };
    let branch = at
        .branch_of(&relpath)
        .unwrap_or_else(|| crate::query::guess_branch(rec));
    let branch = if at.branch(&branch).is_none() {
        DEFAULT_BRANCH.to_string()
    } else {
        branch
    };
    let changed = add_to_branch(&mut at, &branch, &relpath);
    let st = stamps();
    let total = at.mapped_names().len();
    at.set("GENERATED_AT", &st.iso_z);
    at.set("RECORD_COUNT", &total.to_string());
    at.set("COVERED_THROUGH", &st.iso_z);
    at.set(
        "COVERED_RECORDS",
        &format!("全部{total}条记录；逐条PATH覆盖以本图各分支PATH并集为准"),
    );
    if changed || !apath.exists() {
        write_if_changed(&apath, &at.render())?;
    }
    crate::cmd_admin::index_upsert(root, &relpath, &rec.title(), &rec.get("READ_WHEN"))?;
    sync_counts(root)
}

/// Remove a record from the atlas and route index.
pub fn forget(root: &Path, lib: Lib, relpath: &str) -> R<()> {
    let apath = root.join(lib.atlas());
    if apath.exists() {
        let mut at = Atlas::load(&apath)?;
        let touched = remove_from_all(&mut at, relpath);
        if !touched.is_empty() {
            let total = at.mapped_names().len();
            at.set("RECORD_COUNT", &total.to_string());
            write_if_changed(&apath, &at.render())?;
        }
    }
    crate::cmd_admin::index_remove(root, relpath)?;
    sync_counts(root)
}

/// Repoint every reference after a rename.
pub fn repoint(root: &Path, lib: Lib, old_rel: &str, new_rel: &str) -> R<()> {
    let apath = root.join(lib.atlas());
    if apath.exists() {
        let mut at = Atlas::load(&apath)?;
        remove_from_all(&mut at, old_rel);
        let branch = at
            .branch_of(old_rel)
            .unwrap_or_else(|| DEFAULT_BRANCH.to_string());
        add_to_branch(&mut at, &branch, new_rel);
        write_if_changed(&apath, &at.render())?;
    }
    crate::cmd_admin::index_remove(root, old_rel)?;
    if let Ok(rec) = Record::parse(&root.join(new_rel), lib) {
        crate::cmd_admin::index_upsert(root, new_rel, &rec.title(), &rec.get("READ_WHEN"))?;
    }
    sync_counts(root)
}

/// Move a record into a different branch.
pub fn move_branch(root: &Path, lib: Lib, relpath: &str, branch: &str) -> R<()> {
    let apath = root.join(lib.atlas());
    let mut at = if apath.exists() {
        Atlas::load(&apath)?
    } else {
        return notfound("地图不存在，先运行 abook refresh");
    };
    remove_from_all(&mut at, relpath);
    add_to_branch(&mut at, branch, relpath);
    write_if_changed(&apath, &at.render())?;
    Ok(())
}

/// Keep the record counts and atlas timestamps in step with the disk, in
/// **both** entry files and in **both** places each file states them.
///
/// This used to touch only `Global.md`, and only its body section. Two
/// consequences, both observed on the real library:
///
/// * `Index.md` still said `CANDIDATE_RECORDS=437` after the map and the
///   directory had moved to 443, so a reader following the documented order
///   (Global -> Index -> Atlas) saw two different answers and no way to tell
///   which one was stale.
/// * `Global.md` states the same count twice: once in its
///   `[MEMORY_FILE_CONTENTS]` header and once in the body's `## Current State`.
///   Only the body was rewritten, so the file contradicted itself, and the header
///   is the one a reader sees first.
///
/// The directory listing decides how many records exist; the map's
/// `RECORD_COUNT` counts what the map can resolve. Those are deliberately *not*
/// forced equal here -- `state` reports the difference instead of hiding it.
pub fn sync_counts(root: &Path) -> R<()> {
    let live = |lib: Lib| -> usize {
        crate::rec::load_lib(root, lib)
            .iter()
            .filter(|r| r.status() != "DELETED")
            .count()
    };
    let canon = live(Lib::Canonical).to_string();
    let cand = live(Lib::Candidate).to_string();
    let mut covered: Vec<(&str, String)> = Vec::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        if let Ok(at) = Atlas::load(&root.join(lib.atlas())) {
            let key = if lib == Lib::Canonical {
                "COVERED_THROUGH_CANONICAL"
            } else {
                "COVERED_THROUGH_CANDIDATE"
            };
            let v = at.get("COVERED_THROUGH");
            if !v.is_empty() {
                covered.push((key, v));
            }
        }
    }
    for name in ["Global.md", "Index.md"] {
        let p = root.join(name);
        if !p.exists() {
            continue;
        }
        let mut g = BookFile::load(&p)?;
        g.set_everywhere("CANONICAL_RECORDS", &canon);
        g.set_everywhere("CANDIDATE_RECORDS", &cand);
        for (k, v) in &covered {
            g.set_everywhere(k, v);
            // The body's Current State spells two of these without the
            // `COVERED_THROUGH_` prefix; keep whichever form is present in step.
            let short = k.trim_start_matches("COVERED_THROUGH_");
            let alias = format!("{short}_ATLAS_COVERED_THROUGH");
            if g.state(&alias).is_some() {
                g.set_state(&alias, v);
            }
        }
        write_if_changed(&p, &g.render())?;
    }
    Ok(())
}
