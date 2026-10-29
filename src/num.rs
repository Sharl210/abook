//! 短编号：对外只用 `C123` / `N45`，完整编号留在磁盘。
//!
//! ## 为什么要短号
//!
//! 完整编号 `C-20261029T101635-f449f353` 是 26 个字符。它出现在索引、
//! 召回结果、变更日志、彼此引用里，每处都付一次 token。更要紧的是它**自带
//! 信息**：`20261029T101635` 是创建时间，`f449f353` 是内容哈希——一条
//! 引用把"什么时候写的、内容指纹是什么"一起交了出去，这在对外材料里
//! 是不该出现的元信息。
//!
//! 短号 `C123` 是 4 个字符，除序号外不含任何语义。
//!
//! ## 为什么用单调序号而不是时间片段
//!
//! 试过"类型+HHMMSS"（`C-101635`）：871 条里撞了 55 次——同一次批量晋升
//! 会在同一秒写下十几条，时间片段根本不唯一。加了哈希就回到原样。
//!
//! 单调序号是唯一能同时满足三条的做法：短、天然唯一、永不变。
//!
//! ## 稳定性
//!
//! 分配表 `.numbers` 一旦写下就只增不改：记录删除后号不回收，新记录拿
//! 下一个空号。这样"某条记录是 C123"这件事在整个生命周期里都成立，
//! 引用不会因为别处删了东西而错位。
//!
//! 代价是号会稀疏（删得多时出现跳号），但那正是它稳定的原因——复用号
//! 会让旧引用指向新记录，比跳号糟得多。

use crate::rec::Lib;
use crate::util::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const NUM_FILE: &str = ".numbers";
const SCHEMA: &str = "1";

/// 短号分配表。键是完整记录 ID，值是短号。
pub struct Numbers {
    /// 首次从库现状建表时为 true，落盘后清掉。
    /// 没有它，建好的表只留在内存里，下次又重建一遍——而重建会按当时的
    /// 文件列表重新发号，等于号会变。
    pub fresh: bool,
    /// 完整 ID → 短号（如 `C123`）
    by_id: HashMap<String, String>,
    /// 短号 → 完整 ID
    by_num: HashMap<String, String>,
    next_c: u32,
    next_n: u32,
}

fn num_path(root: &Path) -> PathBuf {
    root.join(NUM_FILE)
}

impl Numbers {
    /// 读分配表；不存在则从当前库现状建一份。
    ///
    /// 首次建立时按记录 ID 排序依次发号——排序只为让结果可复现，
    /// 不承担任何语义。建好之后就只增不改。
    pub fn load_or_build(root: &Path) -> Numbers {
        if let Some(n) = Self::load(root) {
            return n;
        }
        let mut n = Numbers {
            fresh: true,
            by_id: HashMap::new(),
            by_num: HashMap::new(),
            next_c: 1,
            next_n: 1,
        };
        for lib in [Lib::Canonical, Lib::Candidate] {
            let dir = root.join(lib.dir());
            let rd = match std::fs::read_dir(&dir) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let mut ids: Vec<String> = rd
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|f| f.ends_with(".md"))
                .map(|f| f.trim_end_matches(".md").to_string())
                .collect();
            ids.sort();
            for id in ids {
                n.assign(&id, lib);
            }
        }
        n
    }

    pub fn load(root: &Path) -> Option<Numbers> {
        let raw = std::fs::read_to_string(num_path(root)).ok()?;
        let mut lines = raw.lines();
        if lines.next()? != SCHEMA {
            return None;
        }
        let mut n = Numbers {
            fresh: false,
            by_id: HashMap::new(),
            by_num: HashMap::new(),
            next_c: 1,
            next_n: 1,
        };
        for line in lines {
            let (num, id) = match line.split_once('\t') {
                Some(v) => v,
                None => continue,
            };
            if num.is_empty() || id.is_empty() {
                continue;
            }
            n.by_id.insert(id.to_string(), num.to_string());
            n.by_num.insert(num.to_string(), id.to_string());
            // 推进 next_*，保证新号不会撞已发出的号
            if let Some(rest) = num.strip_prefix('C') {
                if let Ok(v) = rest.parse::<u32>() {
                    n.next_c = n.next_c.max(v + 1);
                }
            } else if let Some(rest) = num.strip_prefix('N') {
                if let Ok(v) = rest.parse::<u32>() {
                    n.next_n = n.next_n.max(v + 1);
                }
            }
        }
        Some(n)
    }

    pub fn save(&mut self, root: &Path) -> R<()> {
        let mut rows: Vec<(&String, &String)> = self.by_id.iter().collect();
        rows.sort_by(|a, b| a.1.cmp(b.1));
        let mut out = String::from(SCHEMA);
        out.push('\n');
        for (id, num) in rows {
            out.push_str(&format!("{num}\t{id}\n"));
        }
        write_atomic(&num_path(root), &out)?;
        self.fresh = false;
        Ok(())
    }

    /// 给一条记录发号。已有则原样返回。
    pub fn assign(&mut self, id: &str, lib: Lib) -> String {
        if let Some(n) = self.by_id.get(id) {
            return n.clone();
        }
        let num = match lib {
            Lib::Canonical => {
                let n = format!("C{}", self.next_c);
                self.next_c += 1;
                n
            }
            Lib::Candidate => {
                let n = format!("N{}", self.next_n);
                self.next_n += 1;
                n
            }
        };
        self.by_id.insert(id.to_string(), num.clone());
        self.by_num.insert(num.clone(), id.to_string());
        num
    }

    pub fn len(&self) -> usize { self.by_id.len() }

    pub fn num_of(&self, id: &str) -> Option<&str> {
        self.by_id.get(id).map(|s| s.as_str())
    }

    pub fn id_of(&self, num: &str) -> Option<&str> {
        self.by_num.get(num).map(|s| s.as_str())
    }

    /// 短号或完整 ID 都接受，统一返回完整 ID。
    pub fn resolve(&self, sel: &str) -> Option<String> {
        if let Some(id) = self.id_of(sel) {
            return Some(id.to_string());
        }
        if self.by_id.contains_key(sel) {
            return Some(sel.to_string());
        }
        None
    }
}

pub fn cmd_num(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let mut nums = Numbers::load_or_build(&root);
    // `--of <短号>`：反查完整编号。模型偶尔确实需要原始 ID
    // （比如与旧记录、外部日志对照），这是唯一的开口。
    if let Some(sel) = a.get("of") {
        // 双向：给短号回完整 ID，给完整 ID 回短号。
        let (kind, value) = match nums.id_of(&sel) {
            Some(id) => ("full", id.to_string()),
            None => match nums.num_of(&sel) {
                Some(n) => ("short", n.to_string()),
                None => return notfound(&format!("没有 {sel} 这个号")),
            },
        };
        if a.has("json") {
            crate::say_line!(
                "{{\"query\":{},\"kind\":{},\"value\":{}}}",
                jstr(&sel),
                jstr(kind),
                jstr(&value)
            );
        } else {
            crate::say_line!("{value}");
        }
        return Ok(0);
    }
    // 列出全部映射（诊断用；正常不该用，因为很长）
    let _ = nums.save(&root);
    let mut rows: Vec<(String, String)> = nums
        .by_id
        .iter()
        .map(|(id, n)| (n.clone(), id.clone()))
        .collect();
    rows.sort_by(|a, b| {
        let ka = (a.0.starts_with('C'), a.0[1..].parse::<u32>().unwrap_or(0));
        let kb = (b.0.starts_with('C'), b.0[1..].parse::<u32>().unwrap_or(0));
        kb.cmp(&ka).then(ka.cmp(&kb))
    });
    for (n, id) in rows {
        crate::say_line!("{n}\t{id}");
    }
    Ok(0)
}

/// 把一行输出里的完整编号换成短号。
///
/// 用在 `list`/`recall` 这类"给人看"的输出上。规则只有一条：
/// `canonical/records/C-20261029T101635-f449f353.md` → `C123`。
///
/// 为什么在输出层做而不是改存储：完整编号是**磁盘上的身份**，改名会
/// 断掉所有已有引用（`CANDIDATE_SOURCE`、`RELATED_IDS`、`SUPERSEDES` 全指着它）。
/// 短号只是显示层的别名，两套并存才对——显示短、存储全，反查由 `--of` 负责。
pub fn shorten_line(nums: &Numbers, line: &str) -> String {
    // 快速跳过：不含 "records/" 的行不需要处理（绝大多数行如此）。
    if !line.contains("records/") {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < line.len() {
        if let Some(rest) = line[i..].strip_prefix("records/") {
            // 取到 `.md` 为止
            let after = i + "records/".len();
            if let Some(dot) = line[after..].find(".md") {
                let id = &line[after..after + dot];
                let end = after + dot + 3; // 含 ".md"
                if let Some(short) = nums.num_of(id) {
                    out.push_str(short);
                    i = end;
                    continue;
                }
            }
            out.push_str(rest);
            i = after;
            continue;
        }
        // 逐字符推进（按 UTF-8 边界）
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&line[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}
