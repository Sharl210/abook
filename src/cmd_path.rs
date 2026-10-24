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
use crate::rec::{Lib, Record};
use crate::util::*;
use std::collections::HashMap;
use std::path::Path;

/// One branch, with the totals it is responsible for.
struct Node {
    name: String,
    parent: String,
    members: Vec<String>,
    bytes: usize,
    lines: usize,
    updated: i64,
}

fn collect(root: &Path, lib: Lib) -> Vec<Node> {
    let ap = root.join(lib.atlas());
    let at = match Atlas::load(&ap) {
        Ok(a) => a,
        Err(_) => return Vec::new(),
    };
    // Index every record once. Reading a branch's files per branch would stat
    // the same record as many times as it appears in a PATH.
    let mut index: HashMap<String, (usize, usize, i64)> = HashMap::new();
    for r in crate::rec::load_lib(root, lib) {
        let v = (r.raw.len(), r.raw.lines().count(), r.updated());
        index.insert(r.file_name(), v);
        index.insert(format!("{}/{}", lib.dir(), r.file_name()), v);
    }
    at.branches
        .iter()
        .map(|b| {
            let mut seen: Vec<String> = Vec::new();
            let (mut bytes, mut lines, mut updated) = (0usize, 0usize, 0i64);
            for p in b.paths() {
                let bare = p.rsplit('/').next().unwrap_or(&p).to_string();
                if seen.contains(&bare) {
                    continue;
                }
                let (by, ln, ts) = index
                    .get(&p)
                    .or_else(|| index.get(&bare))
                    .copied()
                    .unwrap_or((0, 0, 0));
                seen.push(bare);
                bytes += by;
                lines += ln;
                if ts > updated {
                    updated = ts;
                }
            }
            Node {
                name: b.name(),
                parent: b.parent(),
                members: seen,
                bytes,
                lines,
                updated,
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
                                "{{\"branch\":{},\"count\":{},\"bytes\":{},\"lines\":{},\"updated\":{},\"when\":{}}}",
                                jstr(&x.name),
                                x.members.len(),
                                x.bytes,
                                x.lines,
                                x.updated,
                                jstr(&short_when(x.updated))
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
            "{{\"record\":{},\"id\":{},\"bytes\":{},\"lines\":{},\"updated\":{},\"when\":{},\"chains\":[{}]}}",
            jstr(&bare),
            jstr(&rec.id()),
            rec.raw.len(),
            rec.raw.lines().count(),
            rec.updated(),
            jstr(&short_when(rec.updated())),
            arr
        );
        return Ok(0);
    }

    println!(
        "{}  ({})",
        bare,
        rec.id()
    );
    println!(
        "  {}  {}  {}",
        human_bytes(rec.raw.len() as u64),
        format!("{}行", rec.raw.lines().count()),
        short_when(rec.updated())
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
            let pad = "  ".repeat(depth + 1);
            match node {
                Some(x) => println!(
                    "{pad}{name}  {}条  {}  {}  {}",
                    x.members.len(),
                    human_bytes(x.bytes as u64),
                    format!("{}行", x.lines),
                    short_when(x.updated)
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
                    "{{\"branch\":{},\"depth\":{d},\"parent\":{},\"count\":{},\"bytes\":{},\"lines\":{},\"updated\":{},\"when\":{}}}",
                    jstr(&n.name),
                    jstr(&n.parent),
                    n.members.len(),
                    n.bytes,
                    n.lines,
                    n.updated,
                    jstr(&short_when(n.updated))
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
            "{pad}{mark} {:<28} {}条  {}  {}  {}",
            n.name,
            n.members.len(),
            human_bytes(n.bytes as u64),
            format!("{}行", n.lines),
            short_when(n.updated)
        );
    }
    println!("共 {} 个分支", out.len());
    Ok(0)
}
