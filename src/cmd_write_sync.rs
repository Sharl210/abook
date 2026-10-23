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

/// Keep Global.md counts and covered timestamps in step with the disk.
pub fn sync_counts(root: &Path) -> R<()> {
    let gp = root.join("Global.md");
    if !gp.exists() {
        return Ok(());
    }
    let live = |lib: Lib| -> usize {
        crate::rec::load_lib(root, lib)
            .iter()
            .filter(|r| r.status() != "DELETED")
            .count()
    };
    let mut g = BookFile::load(&gp)?;
    g.set_state("CANONICAL_RECORDS", &live(Lib::Canonical).to_string());
    g.set_state("CANDIDATE_RECORDS", &live(Lib::Candidate).to_string());
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
    write_if_changed(&gp, &g.render())?;
    Ok(())
}
