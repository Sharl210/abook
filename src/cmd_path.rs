//! `path` — where does this record live in the branch tree?
//!
//! A flat map answers "what branches exist". It does not answer the question a
//! reader actually has once they are holding one record: *which* branch owns
//! this, what sits above it, and how big is each level. Without that, going from
//! a record back to its context means reading both Atlas files and matching
//! PATHs by hand -- which is exactly the manual bookkeeping this tool exists to
//! remove.
//!
//! Both directions come from one function: given a record it walks upward;
//! given a branch it walks downward.

use crate::atlas::Atlas;
use crate::cmd_view::{count_lines, meta_json};
use crate::rec::{Lib, Record};
use crate::util::*;
use std::collections::HashMap;
use std::path::Path;

/// One branch, with the totals it is responsible for.
struct Node {
    name: String,
    parent: String,
    members: Vec<String>,
    /// `None` = 这条分支的成员一条都没量到（PATH 指向已改名的文件）。
    /// 不是 0 字节 0 行——那是"这条分支是空的"，与"量不出来"是两件事。
    bytes: Option<u64>,
    lines: Option<usize>,
    updated: i64,
    attach_count: usize,
    attach_bytes: u64,
}

fn collect(root: &Path, lib: Lib) -> Vec<Node> {
    let ap = root.join(lib.atlas());
    let at = match Atlas::load(&ap) {
        Ok(a) => a,
        Err(_) => return Vec::new(),
    };
    // Index every record once. Reading a branch's files per branch would stat
    // the same record as many times as it appears in a PATH.
    let mut index: HashMap<String, (usize, usize, i64, usize, u64)> = HashMap::new();
    for r in crate::rec::load_lib(root, lib) {
        let a = crate::attach::scan(&r.path);
        let v = (
            r.raw.len(),
            count_lines(&r.raw),
            r.updated(),
            a.count(),
            a.bytes,
        );
        index.insert(r.file_name(), v);
        index.insert(format!("{}/{}", lib.dir(), r.file_name()), v);
    }
    at.branches
        .iter()
        .map(|b| {
            let mut seen: Vec<String> = Vec::new();
            let (mut bytes, mut lines, mut updated) = (0u64, 0usize, 0i64);
            let mut measured = 0usize;
            let (mut ac, mut ab) = (0usize, 0u64);
            for p in b.paths() {
                let bare = p.rsplit('/').next().unwrap_or(&p).to_string();
                if seen.contains(&bare) {
                    continue;
                }
                seen.push(bare.clone());
                // PATH 指向的文件不在盘上（记录改过名）：仍是成员，但量不出体积。
                let (by, ln, ts, c, b2) = match index.get(&p).or_else(|| index.get(&bare)) {
                    Some(v) => *v,
                    None => continue,
                };
                measured += 1;
                bytes += by as u64;
                lines += ln;
                ac += c;
                ab += b2;
                if ts > updated {
                    updated = ts;
                }
            }
            let unmeasured = measured == 0 && !seen.is_empty();
            Node {
                name: b.name(),
                parent: b.parent(),
                members: seen,
                bytes: if unmeasured { None } else { Some(bytes) },
                lines: if unmeasured { None } else { Some(lines) },
                updated,
                attach_count: ac,
                attach_bytes: ab,
            }
        })
        .collect()
}

/// Every branch holding this record, nearest first, then the chain above each.
fn chains_for(nodes: &[Node], record: &str) -> Vec<Vec<String>> {
    let by_name: HashMap<&str, &Node> = nodes.iter().map(|n| (n.name.as_str(), n)).collect();
    let mut out: Vec<Vec<String>> = Vec::new();
    for n in nodes.iter().filter(|n| n.members.iter().any(|m| m == record)) {
        let mut chain = vec![n.name.clone()];
        let mut cur = n;
        // Depth cap: a PARENT cycle would otherwise loop forever. Cycle
        // detection is by name, not by depth, so a legitimate deep tree still
        // walks all the way up.
        let mut seen = vec![n.name.clone()];
        loop {
            if cur.parent.is_empty() {
                break;
            }
            if seen.contains(&cur.parent) {
                chain.push(format!("{}（循环）", cur.parent));
                break;
            }
            match by_name.get(cur.parent.as_str()) {
                Some(p) => {
                    chain.push(p.name.clone());
                    seen.push(p.name.clone());
                    cur = p;
                }
                None => {
                    // The parent is named but no card carries it. Report the
                    // dangling reference rather than pretending the chain ends.
                    chain.push(format!("{}（未找到）", cur.parent));
                    break;
                }
            }
        }
        out.push(chain);
    }
    out
}

pub fn cmd_path(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录或分支名")?;

    let mut nodes: Vec<Node> = Vec::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        nodes.extend(collect(&root, lib));
    }
    if nodes.is_empty() {
        return notfound("两个库都没有地图；先运行 `abook refresh --apply`".to_string());
    }

    // A selector may be a branch name. That is the downward direction, and it
    // is the same command because a reader holding a branch name has the same
    // question in the opposite order.
    if let Some(n) = nodes.iter().find(|n| n.name == sel) {
        return show_subtree(a, &nodes, n);
    }

    // Otherwise treat it as a record: resolve the selector to a real file name,
    // so an ID and a full path both work.
    let (path, lib) = match crate::rec::resolve_record(&root, &sel) {
        Ok(v) => v,
        Err(e) => return Err(e),
    };
    let rec = Record::parse(&path, lib)?;
    let bare = rec.file_name();
    // 文字域与文件域各自的量：正文占上下文，附件只在真取时才占。
    let own = crate::attach::scan(&path);

    let chains = chains_for(&nodes, &bare);
    if a.has("json") {
        let arr = chains
            .iter()
            .map(|c| {
                let items = c
                    .iter()
                    .map(|n| {
                        let node = nodes.iter().find(|x| x.name == *n);
                        match node {
                            Some(x) => format!(
                                "{{\"branch\":{},\"count\":{},{},\"attach_count\":{},\"attach_bytes\":{}}}",
                                jstr(&x.name),
                                x.members.len(),
                                meta_json(x.updated, x.bytes, x.lines),
                                x.attach_count,
                                x.attach_bytes
                            ),
                            None => format!("{{\"branch\":{},\"missing\":true}}", jstr(n)),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!("[{items}]")
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"record\":{},\"id\":{},{},\"attach_count\":{},\"attach_bytes\":{},\"chains\":[{}]}}",
            jstr(&bare),
            jstr(&rec.id()),
            meta_json(rec.updated(), Some(rec.raw.len() as u64), Some(count_lines(&rec.raw))),
            own.count(),
            own.bytes,
            arr
        );
        return Ok(0);
    }

    println!(
        "{}  ({})",
        bare,
        rec.id()
    );
    // 列序统一成 `时间 体积 行数`（与 `row_meta` 一致），附件这类命令特有列
    // 接在后面。这里原来是 `体积 行数 时间`，四个命令四种排列。
    println!(
        "{}  {}{}",
        row_meta(rec.updated(), Some(rec.raw.len() as u64), Some(count_lines(&rec.raw))),
        if own.is_empty() {
            String::new()
        } else {
            format!("  {}件附件 共{}", own.count(), human_bytes(own.bytes))
        },
        if own.is_empty() {
            String::new()
        } else {
            format!("（{}）", own.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default())
        }
    );
    if chains.is_empty() {
        println!("  不属于任何分支（地图未收录这条记录）");
        // Exit 3 matches "found the record, found no answer": a caller
        // distinguishing "no such record" (also 3) from this should look at the
        // text, but the common case is a script asking "is it mapped yet".
        return Ok(3);
    }
    if chains.len() > 1 {
        println!("  属于 {} 个分支（同一记录可被多处收录）：", chains.len());
    }
    for (i, c) in chains.iter().enumerate() {
        if chains.len() > 1 {
            println!("  ── 分支链 {} ──", i + 1);
        }
        // Printed nearest-first, so the first line is the branch that owns the
        // record most directly.
        for (depth, name) in c.iter().enumerate() {
            let node = nodes.iter().find(|x| x.name == *name);
            // 前缀占第 0 列，缩进放在它右边，叶子行的层数从 0 起算，与 `tree` 一致。
            let pad = "  ".repeat(depth);
            match node {
                // 元信息前缀从第 0 列起，缩进放到它右边：`path` 与 `tree`/`list`
                // 并排看时前三列对得齐。
                Some(x) => println!(
                    "{}  {pad}{name}  {}条  {}",
                    row_meta(x.updated, x.bytes, x.lines),
                    x.members.len(),
                    if x.attach_count == 0 {
                        String::new()
                    } else {
                        format!("{}件{}", x.attach_count, human_bytes(x.attach_bytes))
                    }
                ),
                None => println!("{pad}{name}"),
            }
        }
    }
    Ok(0)
}

fn show_subtree(a: &Args, nodes: &[Node], start: &Node) -> R<i32> {
    let mut out: Vec<(usize, &Node)> = Vec::new();
    fn walk<'a>(
        nodes: &'a [Node],
        name: &str,
        depth: usize,
        seen: &mut Vec<String>,
        out: &mut Vec<(usize, &'a Node)>,
    ) {
        if seen.contains(&name.to_string()) {
            return;
        }
        seen.push(name.to_string());
        if let Some(n) = nodes.iter().find(|x| x.name == name) {
            out.push((depth, n));
            let mut kids: Vec<&Node> = nodes.iter().filter(|c| c.parent == name).collect();
            kids.sort_by(|x, y| y.bytes.cmp(&x.bytes));
            for k in kids {
                walk(nodes, &k.name, depth + 1, seen, out);
            }
        }
    }
    walk(nodes, &start.name, 0, &mut Vec::new(), &mut out);

    if a.has("json") {
        let items = out
            .iter()
            .map(|(d, n)| {
                format!(
                    "{{\"branch\":{},\"depth\":{d},\"parent\":{},\"count\":{},{}}}",
                    jstr(&n.name),
                    jstr(&n.parent),
                    n.members.len(),
                    meta_json(n.updated, n.bytes, n.lines)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!("{{\"root\":{},\"nodes\":[{items}]}}", jstr(&start.name));
        return Ok(0);
    }
    println!("子树 {}", start.name);
    for (d, n) in &out {
        let pad = "  ".repeat(*d);
        let mark = if *d == 0 { "▸" } else { "·" };
        println!(
            "{}  {pad}{mark} {:<28} {}条  {}",
            row_meta(n.updated, n.bytes, n.lines),
            n.name,
            n.members.len(),
            if n.attach_count == 0 {
                String::new()
            } else {
                format!("{}件{}", n.attach_count, human_bytes(n.attach_bytes))
            }
        );
    }
    println!("共 {} 个分支", out.len());
    Ok(0)
}
