//! `branch`, `recount` and `describe` — the three remaining gaps.
//!
//! `atlas` could only *show* the map: there was no way to add, rename or drop
//! a branch, which meant the map was effectively read-only even though the
//! branch cards are ordinary text. `recount` exists because `COUNT=` drifting
//! away from what is actually on disk is the single most common way this
//! library goes quietly wrong — the number looks authoritative and is wrong.
//! `describe` answers "what may I call, with which parameters" without making
//! an assistant parse the help prose, which is what the prompt chapter will
//! lean on once the whole memory chapter is replaced by a call sheet.

use crate::atlas::{Atlas, Branch};
use crate::paging::Page;
use crate::rec::{Lib, Record};
use crate::util::*;
use std::path::Path;


/// `branch` — inspect and edit the branch cards of a map.
pub fn cmd_branch(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sub = a.pos_or(0, "list");
    let libs = libs_from(a);

    match sub.as_str() {
        "list" | "ls" | "" => {
            let mut js: Vec<String> = Vec::new();
            for lib in &libs {
                let p = root.join(lib.atlas());
                if !p.exists() {
                    continue;
                }
                let at = Atlas::load(&p)?;
                if a.has("json") {
                    let items: Vec<String> = at
                        .branches
                        .iter()
                        .map(|b| {
                            format!(
                                "{{\"name\":{},\"count\":{},\"declared\":{},\"paths\":{},\"revision\":{},\"keys\":{}}}",
                                jstr(&b.name()),
                                b.count(),
                                // `COUNT` can be present with an empty value in
                                // real maps (`unclassified` had `COUNT=`), and
                                // splicing that straight into the JSON produced
                                // `"declared":,` -- which no parser accepts. A
                                // missing declaration is null, not 0: 0 would
                                // claim the branch is known to be empty.
                                match b.get("COUNT").trim().parse::<i64>() {
                                    Ok(n) => n.to_string(),
                                    Err(_) => "null".to_string(),
                                },
                                b.paths().len(),
                                jstr(&b.get("REVISION")),
                                jstr(&b.get("KEYS"))
                            )
                        })
                        .collect();
                    let all = items.len();
                    let (shown, pg) = crate::paging::page_json(a, items);
                    let body = shown.join(",");
                    js.push(format!(
                        "{{\"lib\":{},\"branches\":{all},{pg},\"items\":[{body}]}}",
                        jstr(lib.name())
                    ));
                    continue;
                }
                // `branch list` advertised `--page`/`--per-page` and ignored
                // them: 33 branch lines came back no matter what the caller
                // asked for. Both forms page now.
                let rows: Vec<String> = at
                    .branches
                    .iter()
                    .map(|b| {
                        let n = b.count();
                        let decl: usize = b.get("COUNT").trim().parse().unwrap_or(0);
                        let flag = if decl == n {
                            ""
                        } else {
                            "  <-- COUNT 与实际不符"
                        };
                        format!("   {:<28} {n:>4} 条{flag}  {}", b.name(), b.get("KEYS"))
                    })
                    .collect();
                let (shown, footer) = crate::paging::page_rows(a, rows);
                crate::say_line!("{}   {} 分支", lib.atlas(), at.branches.len());
                for r in &shown {
                    crate::say_line!("{r}");
                }
                if !footer.is_empty() {
                    crate::say_line!("{footer}");
                }
            }
            if a.has("json") {
                crate::say_line!("{{\"libs\":[{}]}}", js.join(","));
            }
            Ok(0)
        }
        "add" | "new" => {
            let name = match a.pos.get(1) {
                Some(n) if !n.trim().is_empty() => n.clone(),
                _ => return usage("branch add 需要分支名"),
            };
            let lib = libs[0];
            let path = root.join(lib.atlas());
            let mut at = if path.exists() {
                Atlas::load(&path)?
            } else {
                let mut t = Atlas::default();
                t.title = format!("{}/Atlas.md", lib.name());
                t
            };
            if at.branch(&name).is_some() {
                return usage(&format!(
                    "分支 `{name}` 已存在；改名用 `abook branch rename {name} <新名>`"
                ));
            }
            let mut b = Branch::default();
            b.set("BRANCH", &name);
            b.set("KEYS", &a.get("keys").unwrap_or_default());
            b.set("COUNT", "0");
            b.set("MAXSIZE", "0");
            b.set("READ_WHEN", &a.get("read-when").unwrap_or_default());
            b.set("REVISION", "1");
            b.set("STATUS", "ACTIVE");
            b.set("PATH", "");
            b.raw = format!(
                "BRANCH={}\nKEYS={}\nCOUNT=0\nMAXSIZE=0\nREAD_WHEN={}\nREVISION=1\nSTATUS=ACTIVE\nPATH=\n\n",
                name,
                a.get("keys").unwrap_or_default(),
                a.get("read-when").unwrap_or_default()
            );
            at.branches.push(b);
            if a.has("dry-run") {
                crate::say_line!("将新增分支 {name} 到 {}", lib.atlas());
                return Ok(0);
            }
            write_atomic(&path, &at.render())?;
            let re = Atlas::load(&path)?;
            if re.branch(&name).is_none() {
                return integrity(&format!("写入后回读不到分支 {name}，已中止"));
            }
            crate::say_line!("已新增分支 {name} -> {}", lib.atlas());
            Ok(0)
        }
        "rename" | "mv" => {
            if a.pos.len() < 3 {
                return usage("branch rename 需要 <旧名> <新名>");
            }
            let (old, new) = (a.pos[1].clone(), a.pos[2].clone());
            let mut done = 0;
            for lib in &libs {
                let path = root.join(lib.atlas());
                if !path.exists() {
                    continue;
                }
                let mut at = Atlas::load(&path)?;
                let idx = match at.branches.iter().position(|b| b.name() == old) {
                    Some(i) => i,
                    None => continue,
                };
                if at.branch(&new).is_some() {
                    return usage(&format!("{} 里已有分支 {new}，拒绝覆盖", lib.atlas()));
                }
                let rev: usize = at.branches[idx].get("REVISION").trim().parse().unwrap_or(1);
                at.branches[idx].set("BRANCH", &new);
                at.branches[idx].set("REVISION", &(rev + 1).to_string());
                if a.has("dry-run") {
                    crate::say_line!("将 {} 里 {old} -> {new}", lib.atlas());
                    done += 1;
                    continue;
                }
                write_atomic(&path, &at.render())?;
                let re = Atlas::load(&path)?;
                if re.branch(&new).is_none() || re.branch(&old).is_some() {
                    return integrity(&format!("{} 改名后回读校验失败", lib.atlas()));
                }
                crate::say_line!("{}: {old} -> {new}", lib.atlas());
                done += 1;
            }
            if done == 0 {
                return usage(&format!("没有找到分支 {old}"));
            }
            Ok(0)
        }
        "delete" | "rm" => {
            let name = match a.pos.get(1) {
                Some(n) => n.clone(),
                None => return usage("branch delete 需要分支名"),
            };
            for lib in &libs {
                let path = root.join(lib.atlas());
                if !path.exists() {
                    continue;
                }
                let mut at = Atlas::load(&path)?;
                let idx = match at.branches.iter().position(|b| b.name() == name) {
                    Some(i) => i,
                    None => continue,
                };
                let n = at.branches[idx].count();
                // A branch holding records is refused unless forced: dropping
                // it would orphan every path inside and the records would look
                // unmapped while still sitting on disk.
                if n > 0 && !a.has("force") {
                    return usage(&format!(
                        "分支 {name} 还有 {n} 条记录；先 `abook branch unmap {name} <记录>` 或加 --force"
                    ));
                }
                if a.has("dry-run") {
                    crate::say_line!("将删除 {} 里分支 {name}（{n} 条）", lib.atlas());
                    continue;
                }
                at.branches.remove(idx);
                write_atomic(&path, &at.render())?;
                let re = Atlas::load(&path)?;
                if re.branch(&name).is_some() {
                    return integrity(&format!("{} 删除后回读仍在", lib.atlas()));
                }
                crate::say_line!("{}: 已删除分支 {name}", lib.atlas());
            }
            Ok(0)
        }
        "map" | "unmap" => {
            if a.pos.len() < 3 {
                return usage(&format!("branch {sub} 需要 <分支> <记录>"));
            }
            let (bname, recq) = (a.pos[1].clone(), a.pos[2].clone());
            let mut hit = false;
            for lib in &libs {
                let path = root.join(lib.atlas());
                if !path.exists() {
                    continue;
                }
                let mut at = Atlas::load(&path)?;
                let idx = match at.branches.iter().position(|b| b.name() == bname) {
                    Some(i) => i,
                    None => continue,
                };
                // Resolve the record against this library so a candidate record
                // cannot be mapped into the canonical map by accident.
                let target = if Path::new(&recq).is_absolute() {
                    Path::new(&recq).to_path_buf()
                } else {
                    root.join(lib.dir()).join(&recq)
                };
                let mut base = recq.rsplit('/').next().unwrap_or(&recq).to_string();
                if !target.exists() {
                    let want = base.trim_end_matches(".md").to_string();
                    let found = list_md(&root.join(lib.dir())).into_iter().find(|p| {
                        let n = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                        n == base
                            || n.trim_end_matches(".md") == want
                            || Record::parse(p, *lib).map(|r| r.id() == want).unwrap_or(false)
                    });
                    match found {
                        Some(p) => base = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or(base),
                        None => continue,
                    }
                }
                let mut ps = at.branches[idx].raw_paths();
                let present = ps.iter().any(|p| p.rsplit('/').next().unwrap_or(p) == base);
                if sub == "map" {
                    if present {
                        crate::say_line!("{}: {base} 已在分支 {bname}", lib.atlas());
                        hit = true;
                        continue;
                    }
                    ps.push(base.clone());
                } else {
                    if !present {
                        crate::say_line!("{}: {base} 不在分支 {bname}", lib.atlas());
                        hit = true;
                        continue;
                    }
                    ps.retain(|p| p.rsplit('/').next().unwrap_or(p) != base);
                }
                at.branches[idx].set_paths_raw(&ps);
                let cnt = at.branches[idx].count().to_string();
                at.branches[idx].set("COUNT", &cnt);
                let rev: usize = at.branches[idx].get("REVISION").trim().parse().unwrap_or(1);
                at.branches[idx].set("REVISION", &(rev + 1).to_string());
                if a.has("dry-run") {
                    crate::say_line!("将 {}: {sub} {base} <-> {bname}", lib.atlas());
                    hit = true;
                    continue;
                }
                write_atomic(&path, &at.render())?;
                crate::say_line!("{}: {base} {} 分支 {bname}", lib.atlas(), if sub == "map" { "加入" } else { "移出" });
                hit = true;
            }
            if !hit {
                return usage(&format!("没有找到分支 {bname} 或记录 {recq}"));
            }
            Ok(0)
        }
        other => usage(&format!(
            "branch 没有子命令 {other}；可用 list/add/rename/delete/map/unmap"
        )),
    }
}

/// `recount` — repair `COUNT=`/`MAXSIZE=` from what is actually on disk.
///
/// This deliberately does *not* re-map records into branches the way `refresh`
/// does. Its only job is to make the numbers true, so it is safe to run when
/// membership editing is in progress.
pub fn cmd_recount(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let libs = libs_from(a);
    let mut js: Vec<String> = Vec::new();
    let mut drift_total = 0usize;

    for lib in &libs {
        let path = root.join(lib.atlas());
        if !path.exists() {
            continue;
        }
        let mut at = Atlas::load(&path)?;
        // Real sizes, keyed by bare name, so a branch cannot claim a size for a
        // file that is gone.
        let mut sizes: Vec<(String, usize)> = Vec::new();
        for p in list_md(&root.join(lib.dir())) {
            if let Ok(m) = std::fs::metadata(&p) {
                let b = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                sizes.push((b, m.len() as usize));
            }
        }
        let mut drift: Vec<(String, String, String)> = Vec::new();
        let mut fresh: Vec<(String, String, String)> = Vec::new();
        for b in at.branches.iter_mut() {
            let ps = b.paths();
            let want_count = b.count();
            let want_max = ps
                .iter()
                .filter_map(|p| {
                    let base = p.rsplit('/').next().unwrap_or(p);
                    sizes.iter().find(|(n, _)| n == base).map(|(_, s)| *s)
                })
                .max()
                .unwrap_or(0);
            let old_count = b.get("COUNT").trim().to_string();
            let old_max = b.get("MAXSIZE").trim().to_string();
            let nc = want_count.to_string();
            let nm = want_max.to_string();
            // "absent" and "wrong" are different reports. The canonical map's
            // trailing `unclassified` card simply carries no COUNT/MAXSIZE line,
            // and the dry run printed that as "有 2 处计数需要修正 … MAXSIZE -> 0",
            // which reads as "your map claims MAXSIZE=something and it is wrong"
            // -- alarming enough to stop a reader from applying the fix at all.
            if old_count != nc {
                let was = if old_count.is_empty() {
                    "COUNT 缺失".to_string()
                } else {
                    format!("COUNT {old_count}")
                };
                drift.push((b.name(), was, nc.clone()));
                b.set("COUNT", &nc);
            }
            if old_max != nm {
                let was = if old_max.is_empty() {
                    "MAXSIZE 缺失".to_string()
                } else {
                    format!("MAXSIZE {old_max}")
                };
                drift.push((b.name(), was, nm.clone()));
                b.set("MAXSIZE", &nm);
            }
            fresh.push((b.name(), nc, nm));
        }
        // A card pointing at nothing on disk is worth naming: it usually means
        // a record was renamed without updating the map.
        let mut dangling: Vec<String> = Vec::new();
        for b in &at.branches {
            for p in b.paths() {
                let base = p.rsplit('/').next().unwrap_or(&p).to_string();
                if !sizes.iter().any(|(n, _)| *n == base) {
                    dangling.push(format!("{}: {base}", b.name()));
                }
            }
        }
        drift_total += drift.len();

        if a.has("json") {
            let d = drift
                .iter()
                .map(|(b, f, t)| {
                    format!("{{\"branch\":{},\"field\":{},\"to\":{}}}", jstr(b), jstr(f), jstr(t))
                })
                .collect::<Vec<_>>()
                .join(",");
            let dg = dangling
                .iter()
                .map(|s| jstr(s))
                .collect::<Vec<_>>()
                .join(",");
            js.push(format!(
                "{{\"lib\":{},\"fixed\":[{}],\"dangling\":[{dg}]}}",
                jstr(lib.atlas()),
                d
            ));
            if !drift.is_empty() && !a.dry_run() {
                write_atomic(&path, &at.render())?;
            }
            continue;
        }

        if drift.is_empty() {
            crate::say_line!("{}: {} 个分支计数都正确", lib.atlas(), fresh.len());
        } else if a.dry_run() {
            crate::say_line!("{}: {} 处计数与目录实测不符（预演，加 --apply 才写）", lib.atlas(), drift.len());
            for (b, f, t) in &drift {
                crate::say_line!("   {b}: {f} -> {t}");
            }
        } else {
            write_atomic(&path, &at.render())?;
            let re = Atlas::load(&path)?;
            let ok = re.branches.iter().all(|b| {
                let want = b.count().to_string();
                b.get("COUNT").trim() == want
            });
            if !ok {
                return integrity(&format!("{} 写入后回读计数仍不一致", lib.atlas()));
            }
            crate::say_line!("{}: 已写回 {} 处计数", lib.atlas(), drift.len());
            for (b, f, t) in &drift {
                crate::say_line!("   {b}: {f} -> {t}");
            }
        }
        if !dangling.is_empty() {
            crate::say_line!("   指向不存在文件的路径 {} 条:", dangling.len());
            for s in dangling.iter().take(10) {
                crate::say_line!("      {s}");
            }
            if dangling.len() > 10 {
                crate::say_line!("      …还有 {} 条", dangling.len() - 10);
            }
        }
    }
    if a.has("json") {
        crate::say_line!("{{\"libs\":[{}],\"drift\":{drift_total}}}", js.join(","));
    }
    Ok(0)
}

/// `describe` — the call contract, machine readable.
///
/// The help text is prose meant for a human. An assistant that must pick a
/// command cannot reliably parse it, and guessing which flags exist is exactly
/// how a caller ends up passing a flag the parser accepts and ignores.
pub fn cmd_describe(a: &Args) -> R<i32> {
    if let Some(one) = a.pos.first() {
        let (cmd, params, summary) = match crate::command_spec(one) {
            Some(x) => x,
            None => {
                return usage(&format!(
                    "没有命令 {one}；运行 `abook commands` 看全部命令名"
                ))
            }
        };
        if a.has("json") {
            crate::say_line!(
                "{{\"command\":{},\"summary\":{},\"usage\":{},\"params\":{},\"paging\":{},\"json\":{}}}",
                jstr(&cmd),
                jstr(&summary),
                jstr(&crate::spec::positionals(&cmd)),
                jstr(&params),
                crate::spec::takes_flag(&cmd, "page") || crate::spec::takes_flag(&cmd, "per-page"),
                crate::command_takes_json(&cmd)
            );
        } else {
            crate::say_line!("{cmd}  {summary}");
            // Positionals first: a caller that reads only one line reads this
            // one, and "which flags exist" does not tell it how to start.
            let pos = crate::spec::positionals(&cmd);
            if !pos.is_empty() {
                crate::say_line!("用法: {cmd} {pos} [选项]");
            } else {
                crate::say_line!("用法: {cmd} [选项]");
            }
            crate::say_line!("参数: {params}");
            // `params` arrives joined with " --", not with commas. Splitting on
            // a comma yielded one single fragment containing the whole list, so
            // every command -- including `list`, whose whole job is paging --
            // was reported as "此命令不分页" while `--page` worked fine. A
            // caller reading that line would stop paginating a 507-record list
            // and load it whole.
            let has = |n: &str| {
                params
                    .split(" --")
                    .any(|p| p.trim().trim_start_matches("--") == n)
            };
            if has("page") || has("per-page") {
                crate::say_line!("分页: 支持 --page / --per-page（默认 30，0=不分页）");
            } else {
                crate::say_line!("分页: 此命令不分页");
            }
            crate::say_line!(
                "JSON: {}",
                if crate::command_takes_json(&cmd) {
                    "支持 --json"
                } else {
                    "此命令无 --json（输出即最终形态）"
                }
            );
        }
        return Ok(0);
    }

    // Every name a caller can actually type, not just the canonical ones.
    //
    // `commands` lists 70 names; this one listed 56, because it enumerated the
    // parameter table (which holds one row per real command) instead of the
    // dispatch table. The 14 aliases -- `add`, `cat`, `find`, `rm`, `map` … --
    // were dispatchable and `describe add` answered fine, but they did not
    // appear in the list, so a caller walking it concluded those names did not
    // exist. Two inventories of the same tool must agree.
    let names: Vec<String> = crate::known_commands()
        .iter()
        .filter(|n| !n.starts_with('-'))
        .map(|n| n.to_string())
        .collect();
    if a.has("json") {
        let items = names
            .iter()
            .map(|n| {
                let canon = crate::spec::canonical(n);
                let (_, params, summary) = crate::command_spec(n.as_str()).unwrap_or((
                    n.clone(),
                    String::new(),
                    String::new(),
                ));
                let alias = if canon == *n {
                    String::new()
                } else {
                    format!(",\"alias_of\":{}", jstr(&canon))
                };
                format!(
                    "{{\"command\":{},\"summary\":{},\"params\":{},\"json\":{}{alias}}}",
                    jstr(n),
                    jstr(&summary),
                    jstr(&params),
                    crate::command_takes_json(n.as_str())
                )
            })
            .collect::<Vec<_>>();
        let (shown, pg) = crate::paging::page_json(a, items);
        crate::say_line!(
            "{{\"count\":{},{pg},\"commands\":[{}]}}",
            shown.len(),
            shown.join(",")
        );
        return Ok(0);
    }
    crate::say_line!("共 {} 条命令。`abook describe <命令>` 看单条参数，`--json` 机器读。", names.len());
    let rows: Vec<String> = names
        .iter()
        .map(|n| {
            let (_, params, summary) = crate::command_spec(n.as_str()).unwrap_or((
                n.clone(),
                String::new(),
                String::new(),
            ));
            let j = if crate::command_takes_json(n.as_str()) { "json" } else { "    " };
            let canon = crate::spec::canonical(n);
            let mark = if canon == *n {
                String::new()
            } else {
                format!("（{canon} 的别名）")
            };
            format!("  {n:<14} {j}  {:<44} {summary}{mark}", truncate_chars(&params, 44))
        })
        .collect();
    let (shown, footer) = crate::paging::page_rows(a, rows);
    for r in &shown {
        crate::say_line!("{r}");
    }
    if !footer.is_empty() {
        crate::say_line!("{footer}");
    }
    Ok(0)
}

/// `leftover` — interrupted writes leave `.tmpXXXXX` files behind.
///
/// A write is only reported as succeeded after the temp file is renamed into
/// place, so anything still named `*.tmpNNNNN` is the residue of a run that
/// died mid-write. They are never valid records and nothing reads them, but
/// they accumulate silently and make a directory listing lie about what is
/// committed. This reports them; `--apply` removes them.
pub fn cmd_leftover(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let apply = a.has("apply");
    let want = libs_from(a);
    let mut found: Vec<(String, u64, String)> = Vec::new();
    let mut scanned = 0usize;
    for dir in crate::rec::all_record_dirs(&root)
        .into_iter()
        .filter(|d| {
            let s = d.to_string_lossy();
            want.iter().any(|l| match l {
                Lib::Canonical => s.contains("canonical"),
                Lib::Candidate => s.contains("candidate"),
            })
        })
    {
        let rd = match std::fs::read_dir(&dir) {
            Ok(x) => x,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            scanned += 1;
            let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            if !name.contains(".tmp") {
                continue;
            }
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            found.push((rel(&root, &p), size, name));
        }
    }
    found.sort();
    let pg = Page::from_args(a, found.len());
    if a.has("json") {
        let items = found
            .iter()
            .skip(pg.start())
            .take(pg.per.min(found.len()))
            .map(|(p, sz, _)| format!("{{\"path\":{},\"bytes\":{sz}}}", jstr(p)))
            .collect::<Vec<_>>()
            .join(",");
        crate::say_line!(
            "{{{},\"scanned\":{scanned},\"total\":{},\"applied\":{apply},\"files\":[{items}]}}",
            pg.json_page_only(),
            found.len()
        );
    } else {
        for (p, sz, _) in found.iter().skip(pg.start()).take(pg.per.min(found.len())) {
            crate::say_line!("  {p}  {sz} 字节");
        }
        crate::say_line!(
            "{}：扫描 {scanned} 个文件，残留 {} 个{}",
            if apply { "已清理" } else { "预演（加 --apply 才删除）" },
            found.len(),
            if found.is_empty() { "" } else { "（中断写入的临时文件，不是记录）" }
        );
        crate::say_line!("{}", pg.bar());
    }
    if apply && !found.is_empty() {
        let mut done = 0usize;
        for (p, _, _) in &found {
            if std::fs::remove_file(root.join(p)).is_ok() {
                done += 1;
            }
        }
        eprintln!("已删除 {done} 个临时文件");
        if done != found.len() {
            return integrity(format!(
                "只删掉 {done}/{} 个临时文件，其余被占用或无权限",
                found.len()
            ));
        }
    }
    Ok(if found.is_empty() { 0 } else if apply { 0 } else { 5 })
}
