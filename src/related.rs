//! `abook related <ID>` —— 从一条记录走到相邻记录。
//!
//! 三类邻居，各自有明确的信息来源：
//!
//! 1. **显式边**：`RELATED_IDS` / `SUPERSEDES` / `PREVIOUS_ID` / `PROMOTED_FROM`。
//!    这些字段早就有，`refs` 命令也早就在查它们是否悬空——缺的只是"能顺着走"。
//!
//! 2. **共享 KEYS/CONCEPTS**，按 IDF 加权。共享「记忆」不算关联（几乎每条都有），
//!    共享「SKILL_ENTRY_BUDGET」才算。这一步把"引用键"当成词表用。
//!
//! 3. **共享硬标识符**：命令名、路径、门名、错误码。这些是记忆里最可靠的锚点
//!    ——它们是原文里的字面量，不依赖记录者当时选词的品味。
//!
//! ## 为什么现算，不建图索引
//!
//! 建一张 `graph/` 落盘表要多维护一处一致性。这库已经踩过 `PROMOTED_FROM`
//! 悬空、候选 ID 改名导致引用断裂的坑，再加一张需要同步的表是给自己找事。
//! 851 条记录的共现计算是毫秒级，不需要预建。
//!
//! ## 边界
//!
//! 图的价值可能比预期小：库里存的是经验，不是事实网络，经验之间的关联本来就稀。
//! 真实收益在"精确锚点查得到"，不在"从 A 走三步到 D"。

use crate::rec::{load_lib, Lib, Record};
use crate::util::*;
use std::collections::HashMap;

/// 一条邻居。
pub struct Neighbor {
    pub why: &'static str,
    pub score: f64,
    pub id: String,
    pub title: String,
}

/// 从正文里抽硬标识符。
///
/// 认四类：大写门名（`SKILL_ENTRY_BUDGET`）、命令名（`abook xxx`）、
/// 相对路径（`canonical/records/...`）、错误码（`E1234` / `RECALL_MISS`）。
/// 不认自然语言——那是 KEYS 的活。
fn anchors(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in text.split(|c: char| {
        c.is_whitespace() || "，。；：、（）【】「」\"'`*|>#[](){}<>,;:!?".contains(c)
    }) {
        let t = tok.trim();
        if t.len() < 5 || t.len() > 80 {
            continue;
        }
        let upper_ratio = t.chars().filter(|c| c.is_ascii_uppercase()).count() as f64
            / t.chars().count().max(1) as f64;
        let looks_gate = t.contains('_') && upper_ratio > 0.5;
        // 相对路径才算锚点。以 `/` 开头的是记录格式里的段标记
        // （`/CANDIDATE_MEMORY`、`/MEMORY_BODY`），每条记录都有，不构成关联。
        let looks_path = t.contains('/')
            && !t.starts_with('/')
            && !t.starts_with('-')
            && t.contains(".md")
            || (t.contains('/') && t.starts_with("src/"))
            || (t.contains('/') && t.starts_with("docs/"));
        let looks_cmd = t.starts_with("abook ");
        let looks_code = t.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
            && t.contains('_')
            && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        // 排除通用词：它们在几乎每条记录里都出现，共享它们不构成关联。
        // 不加这一步，任何两条记录都能靠 `canonical/records` 或 `abook`
        // 拿到一样的分——实测所有邻居都是 108 分，图没有信息量。
        // 排除两类：
        //   ① 路径与文件名这类"人人都写"的通用串；
        //   ② **记录格式自身的字段名**——`CANDIDATE_MEMORY`、`MEMORY_BODY`、
        //      `AUTHORITY=...` 这些是每条记录都有的抬头，把它们当锚点会让
        //      任意两条记录都"相关"，图彻底没有信息量。实测未排除时
        //      所有邻居都是同一个分（108.0），全是无关记录。
        const COMMON: &[&str] = &[
            "canonical/records", "candidate/records", "abook", "SKILL.md",
            "Global.md", "Index.md", "Atlas.md", "README.md", "Cargo.toml",
            "CANDIDATE_MEMORY", "MEMORY_BODY", "CANDIDATE_MEMORY_V2",
            "Asia/Shanghai", "Asia", "UTC", "Shanghai",
        ];
        if COMMON.contains(&t) {
            continue;
        }
        // 字段赋值形态（`X=Y`）是抬头，不是技术标识符
        if t.contains('=') {
            continue;
        }
        // 结尾的省略号说明是从散文里截断的半截词
        if t.ends_with('…') || t.ends_with("...") {
            continue;
        }
        // 最后一道闸：格式段标记（`/MEMORY_BODY`、`/CANDIDATE_MEMORY`）
        // 会被 looks_code 规则命中（去掉斜杠后是合法的全大写标识符形态），
        // 所以必须在这里再拦一次。它们出现在每条记录里，共享它们等于人人相关。
        let stripped = t.trim_start_matches('/');
        if stripped.starts_with("MEMORY_") || stripped == "CANDIDATE_MEMORY" {
            continue;
        }
        if looks_gate || looks_path || looks_cmd || looks_code {
            out.push(t.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn keys_of(rec: &Record) -> Vec<String> {
    // KEYS 字段里存的常常是**整句**而不是词——实测：
    //   KEYS=abook 索引只管条目不管附件：四条理由,以及附件扫描其实不慢
    // 直接按逗号切出来的两半是两个句子，两条记录的 KEYS 永远不可能相等，
    // 那这一整路"共享引用键"就等于没实现。
    //
    // 所以对 KEYS 再做一次分词（复用 semantic 的 2-gram，它本来就是为中文
    // 无词典分词写的），把句子拆成可比较的词。
    //
    // 不改历史数据：旧库的 KEYS 长什么样是既成事实，能读出来就行。
    let mut v: Vec<String> = Vec::new();
    for field in ["KEYS", "CONCEPTS"] {
        for item in split_list(&rec.get(field)) {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            for t in crate::semantic::tokenize(item) {
                if t.chars().count() >= 2 {
                    v.push(t);
                }
            }
        }
    }
    v.sort();
    v.dedup();
    v
}

/// 找出与 `target` 相邻的记录。
pub fn neighbors(root: &std::path::Path, target: &str, limit: usize) -> R<(String, Vec<Neighbor>)> {
    let mut all: Vec<Record> = Vec::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        all.extend(load_lib(root, lib));
    }
    let me = all
        .iter()
        .find(|r| {
            let stem = r.file_name().trim_end_matches(".md").to_string();
            // 允许用 ID 前缀查：时间戳部分已经能唯一定位，
            // 让人每次抄全 "时间戳-哈希" 没有必要。
            stem == target || stem.starts_with(&format!("{target}-")) || r.get("ID") == target
        })
        .ok_or_else(|| EE::NotFound(format!("没有找到记录 {target}")))?;

    // 用解析后的**完整** stem 排自己，不是用户输入的短 ID。
    // 否则 `related N-xxx` 的第一条邻居是它自己——实测踩到。
    let me_stem = me.file_name().trim_end_matches(".md").to_string();
    let mut out: Vec<Neighbor> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    seen.insert(me_stem.clone());

    // ---- 1. 显式边 ----
    for f in ["RELATED_IDS", "SUPERSEDES", "PREVIOUS_ID", "PROMOTED_FROM"] {
        for one in split_list(&me.get(f)) {
            let one = one.trim().to_string();
            if one.is_empty() || !seen.insert(one.clone()) {
                continue;
            }
            if let Some(r) = all.iter().find(|r| r.file_name().trim_end_matches(".md") == one) {
                out.push(Neighbor {
                    why: "显式引用",
                    score: 100.0,
                    id: one,
                    title: r.title(),
                });
            }
        }
    }

    // ---- 2. 共享 KEYS，按 IDF 加权 ----
    //
    // 关键是 IDF：共享「记忆」不算关联，共享「SKILL_ENTRY_BUDGET」才算。
    // 不做这一步的话，任何两条记录都能靠一个泛词"关联"上，图就没有信息量。
    let mut key_df: HashMap<String, usize> = HashMap::new();
    for r in &all {
        for k in keys_of(r) {
            *key_df.entry(k).or_insert(0) += 1;
        }
    }
    let n = all.len().max(1) as f64;
    let my_keys = keys_of(me);
    let idf = |k: &str| -> f64 {
        let df = *key_df.get(k).unwrap_or(&1) as f64;
        (((n - df + 0.5) / (df + 0.5)).ln()).max(0.0)
    };

    // ---- 3. 共享硬标识符 ----
    let my_anchors = anchors(&me.raw);

    for r in &all {
        let rid = r.file_name().trim_end_matches(".md").to_string();
        if seen.contains(&rid) {
            continue;
        }
        let mut sc = 0.0f64;
        let mut why: &'static str = "";

        let shared_keys: f64 = keys_of(r)
            .iter()
            .filter(|k| my_keys.contains(k))
            .map(|k| idf(k))
            .sum();
        if shared_keys > 0.0 {
            sc += shared_keys * 8.0;
            why = "共享引用键";
        }

        let shared_anchors = anchors(&r.raw)
            .iter()
            .filter(|a| my_anchors.contains(a))
            .count();
        if shared_anchors > 0 {
            sc += shared_anchors as f64 * 12.0;
            why = "共享硬标识符";
        }

        if sc > 0.0 && seen.insert(rid.clone()) {
            out.push(Neighbor {
                why,
                score: sc,
                id: rid,
                title: r.title(),
            });
        }
    }

    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(limit);
    Ok((me.title(), out))
}

pub fn cmd_related(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let id = match a.pos.first() {
        Some(v) => v.clone(),
        None => return usage("related 需要记录 ID"),
    };
    let limit = a.num("limit").unwrap_or(12) as usize;
    let (title, ns) = neighbors(&root, &id, limit)?;

    if a.has("json") {
        let rows = ns
            .iter()
            .map(|n| {
                format!(
                    "{{\"id\":{},\"title\":{},\"why\":{},\"score\":{:.2}}}",
                    jstr(&n.id),
                    jstr(&n.title),
                    jstr(n.why),
                    n.score
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        crate::say_line!("{{\"id\":{},\"title\":{},\"neighbors\":[{}]}}", jstr(&id), jstr(&title), rows);
        return Ok(0);
    }

    crate::say_line!("{id}  {title}");
    if ns.is_empty() {
        crate::say_line!("  没有相邻记录。");
        return Ok(0);
    }
    for n in &ns {
        crate::say_line!("  {:<14} {:>6.1}  {}  {}", n.why, n.score, n.id, n.title);
    }
    Ok(0)
}
