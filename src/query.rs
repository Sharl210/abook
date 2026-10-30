//! Recall: tiered matching over record headers, then precision filtering.
//!
//! Tier 1: literal match in KEYS / CONCEPTS / ENTITIES / TITLE
//! Tier 2: semantic-ish match in READ_WHEN
//! Tier 3: branch summary match
//!
//! Every hit is then checked against NOT_FOR and entity/scope, and a rejected
//! hit carries its rejection reason so callers can see why it was dropped.

use crate::atlas::{Atlas, DEFAULT_BRANCH};
use crate::rec::{load_lib, Lib, Record};
use crate::util::*;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Lexical = 1,
    ReadWhen = 2,
    Ngram = 3,
    Branch = 4,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Lexical => "词面",
            Tier::ReadWhen => "READ_WHEN",
            Tier::Ngram => "片段回退",
            Tier::Branch => "分支摘要",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub lib: Lib,
    pub path: String,
    pub title: String,
    pub tier: Tier,
    pub score: i32,
    pub reason: String,
    pub rejected: Option<String>,
    pub record: Option<Record>,
    /// When the record was last touched.
    ///
    /// Display only: recall is ordered by match strength, and that stays true.
    /// But a match with no date cannot be placed in time -- a reader deciding
    /// whether to trust a hit needs to know if it is from this week or from a
    /// year ago, and the score says nothing about that.
    pub updated: i64,
    /// Size of the record file, so a caller can decide whether to page it.
    pub bytes: usize,
    /// Line count of the record file.
    ///
    /// Readers navigate by line number ("look at line 40"), so a listing that
    /// gives size but not lines still leaves them guessing how far in a
    /// location is. Both are cheap to compute from text already in hand.
    pub lines: usize,
    /// Attachment folder beside the record: count and total bytes.
    ///
    /// Kept separate from `bytes`/`lines` because the two halves cost different
    /// things. The record body is paid for on every read; attachments are only
    /// paid for when taken. Folding them into one number would hide the case
    /// that matters -- 2 KB of prose next to 40 MB of video.
    pub attach_count: usize,
    pub attach_bytes: u64,
}

/// Split a query into terms: whitespace / comma, keep CJK runs intact.
pub fn terms(q: &str) -> Vec<String> {
    // 切分规则：先按分隔符切开，再在**中英边界**处补切一刀。
    //
    // 只按空白/逗号切的时候，`abook并发写入` 会被当成一个 8 字 token 去做
    // 字面匹配，而记录里写的是 `abook 并发写入`——一个空格之差，召回全灭。
    // 实测：带空格能命中，去掉空格就"无命中"。
    //
    // 这是同一个老问题的第二处：之前修的是 BM25 那一路（整句中文做 token
    // 必然全灭），字面这一路漏了。中文查询里中英混写极常见（工具名+中文），
    // 不补这一刀就等于一半查询失效。
    let is_cjk = |c: char| (c as u32) >= 0x3400;
    let mut out: Vec<String> = Vec::new();
    for part in q.split(|c: char| c.is_whitespace() || c == ',' || c == '，' || c == ';') {
        let t = part.trim();
        if t.is_empty() {
            continue;
        }
        // 在中英交界处切：`abook并发写入` → `abook` + `并发写入`
        let chars: Vec<char> = t.chars().collect();
        let mut start = 0usize;
        for i in 1..chars.len() {
            if is_cjk(chars[i]) != is_cjk(chars[i - 1]) {
                let piece: String = chars[start..i].iter().collect();
                if !piece.is_empty() {
                    out.push(piece);
                }
                start = i;
            }
        }
        let tail: String = chars[start..].iter().collect();
        if !tail.is_empty() {
            out.push(tail);
        }
    }
    if out.is_empty() && !q.trim().is_empty() {
        out.push(q.trim().to_string());
    }
    out
}

/// Score one record against the query terms.
pub fn score_record(rec: &Record, qs: &[String]) -> Option<(i32, Tier, String)> {
    let mut best: Option<(i32, Tier, String)> = None;
    for q in qs {
        let mut sc = 0;
        let mut tier = Tier::Branch;
        let mut why = Vec::new();
        if ci_contains(&rec.get("KEYS"), q) {
            sc += 100;
            tier = Tier::Lexical;
            why.push("KEYS");
        }
        if ci_contains(&rec.get("CONCEPTS"), q) {
            sc += 80;
            tier = Tier::Lexical;
            why.push("CONCEPTS");
        }
        if ci_contains(&rec.get("ENTITIES"), q) {
            sc += 70;
            tier = Tier::Lexical;
            why.push("ENTITIES");
        }
        if ci_contains(&rec.title(), q) {
            sc += 60;
            if tier == Tier::Branch {
                tier = Tier::Lexical;
            }
            why.push("TITLE");
        }
        if ci_contains(&rec.get("ID"), q) {
            sc += 90;
            tier = Tier::Lexical;
            why.push("ID");
        }
        if ci_contains(&rec.get("READ_WHEN"), q) {
            sc += 40;
            if tier == Tier::Branch {
                tier = Tier::ReadWhen;
            }
            why.push("READ_WHEN");
        }
        if ci_contains(&rec.get("SUMMARY"), q) {
            sc += 20;
            if tier == Tier::Branch {
                tier = Tier::ReadWhen;
            }
            why.push("SUMMARY");
        }
        if ci_contains(&rec.body, q) {
            sc += 10;
            why.push("BODY");
        }
        if sc > 0 {
            let reason = why.join("+");
            match &best {
                Some((b, _, _)) if *b >= sc => {}
                _ => best = Some((sc, tier, reason)),
            }
        }
    }
    best
}

/// Character n-gram windows for a term. Latin runs stay whole; CJK runs are
/// cut into all 2-grams so a colloquial shorthand can still reach a record
/// that spells the term out in full.
pub fn ngrams(term: &str) -> Vec<String> {
    let chars: Vec<char> = term.chars().collect();
    let mut out: Vec<String> = Vec::new();
    if chars.len() < 2 {
        return out;
    }
    let cjk = |c: char| (c as u32) >= 0x3400;
    // split into runs of CJK and non-CJK
    let mut i = 0;
    while i < chars.len() {
        if cjk(chars[i]) {
            let start = i;
            while i < chars.len() && cjk(chars[i]) {
                i += 1;
            }
            let run = &chars[start..i];
            if run.len() == 1 {
                out.push(run.iter().collect());
            } else {
                for w in run.windows(2) {
                    out.push(w.iter().collect());
                }
            }
        } else {
            let start = i;
            while i < chars.len() && !cjk(chars[i]) {
                i += 1;
            }
            let run: String = chars[start..i].iter().collect();
            let t = run.trim();
            if t.len() >= 2 {
                out.push(t.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Fallback scoring by n-gram overlap, used only when the whole-term pass
/// found nothing for that term.
pub fn score_ngram(rec: &Record, qs: &[String]) -> Option<(i32, Tier, String)> {
    let mut total = 0i32;
    let mut hits: Vec<String> = Vec::new();
    // Coverage gate: a single stray fragment is noise, not a recall. Require
    // at least half of the windows (and never fewer than two) to land.
    let mut all_grams: Vec<String> = Vec::new();
    for q in qs {
        all_grams.extend(ngrams(q));
    }
    all_grams.sort();
    all_grams.dedup();
    let need = if all_grams.len() <= 1 {
        1
    } else {
        (all_grams.len() + 1) / 2
    };
    for q in qs {
        for g in ngrams(q) {
            let mut sc = 0;
            if ci_contains(&rec.get("KEYS"), &g) {
                sc += 12;
            }
            if ci_contains(&rec.get("CONCEPTS"), &g) {
                sc += 10;
            }
            if ci_contains(&rec.get("ENTITIES"), &g) {
                sc += 8;
            }
            if ci_contains(&rec.title(), &g) {
                sc += 7;
            }
            if ci_contains(&rec.get("READ_WHEN"), &g) {
                sc += 4;
            }
            if ci_contains(&rec.get("SUMMARY"), &g) {
                sc += 3;
            }
            if ci_contains(&rec.body, &g) {
                sc += 2;
            }
            if sc > 0 {
                total += sc;
                if !hits.contains(&g) {
                    hits.push(g);
                }
            }
        }
    }
    if total == 0 || hits.len() < need {
        return None;
    }
    let reason = format!("片段 {}/{} {}", hits.len(), all_grams.len(), hits.join("/"));
    Some((total, Tier::Ngram, reason))
}

/// Precision check: NOT_FOR exclusion and explicit scope.
fn precision(rec: &Record, qs: &[String]) -> Option<String> {
    let nf = rec.get("NOT_FOR");
    if nf.is_empty() {
        return None;
    }
    for q in qs {
        if ci_contains(&nf, q) {
            return Some(format!("命中 NOT_FOR: {q}"));
        }
    }
    None
}

#[derive(Clone, Debug, Default)]
pub struct RecallResult {
    pub query: String,
    pub hits: Vec<Hit>,
    pub rejected: Vec<Hit>,
    pub tiers_used: Vec<Tier>,
    pub scanned: usize,
    pub status: &'static str,
}

/// Run recall across one or both libraries.
pub fn recall(
    root: &Path,
    q: &str,
    libs: &[Lib],
    limit: usize,
    include_promoted: bool,
    with_body: bool,
) -> RecallResult {
    let qs = terms(q);
    if std::env::var("ABOOK_DEBUG_TERMS").is_ok() {
        eprintln!("[TERMS] {q:?} → {qs:?}");
    }
    let mut res = RecallResult {
        query: q.to_string(),
        status: "RECALL_MISS",
        ..Default::default()
    };

    // ---- 索引快路径 ----
    //
    // 索引存在且与磁盘对得上时，整条召回都在索引上完成：不读记录正文、
    // 不解析 Markdown、不建内存索引。这是把 O(n) 换成查表的关键一步。
    //
    // 只有命中并截断后的那几条才回磁盘读正文——因为调用方可能要看摘要，
    // 而正文按定义只有几条。这就是"常数级"的来源：磁盘读的次数取决于
    // limit，不取决于库里有几条记录。
    //
    // 索引不可用（首次、被删、版本不符、对不上）时走下面的老路径，
    // 行为与之前完全一致。`rm -rf .index/` 之后一切照常，是设计承诺。
    // 浅加载：只读 meta + postings，不读 docs 分片。
    // docs 按需读——只取命中的那几条所在的分片，磁盘读次数取决于 limit
    // 而不是库里有多少条。这是常数级的来源。
    if let Some(disk) = crate::idx::Index::load_shallow(root, q) {
        if disk.total_docs > 0 {
            return recall_via_index(root, q, &qs, &disk, limit, with_body);
        }
    }

    let mut all: Vec<(Lib, Record)> = Vec::new();
    for lib in libs {
        for rec in load_lib(root, *lib) {
            all.push((*lib, rec));
        }
    }
    res.scanned = all.len();

    // promoted candidates are not part of normal auto-recall
    let mut pool: Vec<(Lib, Record)> = Vec::new();
    for (lib, rec) in all {
        // Deleted records leave the current view; they stay on disk so the
        // deletion can be reverted, and are reachable only via `show`.
        if rec.status() == "DELETED" {
            continue;
        }
        if lib == Lib::Candidate && !include_promoted {
            if rec.promotion.is_some() || rec.get("PROMOTED_TO").len() > 0 {
                continue;
            }
        }
        pool.push((lib, rec));
    }

    let mut hits: Vec<Hit> = Vec::new();

    // ---- 四路并行打分，RRF 融合 ----
    //
    // 以前是四层**互斥降级**：整词有一条命中，n-gram 与分支摘要就永不启用。
    // 后果是「一条记录正文里偶然出现查询词，BODY 给 10 分就算命中」，
    // 于是 hits 非空、后两层不跑——一个 10 分的噪声能把 100 分的正确记录挡在门外。
    //
    // 现在四路各出各的分，再用 RRF（倒数排名融合）合并。选 RRF 而不是加权求和，
    // 是因为它只看排名、不看分数绝对值：词面分和 BM25 分不同量纲，直接相加
    // 需要一个永远调不完的权重，而排名是可比的。
    //
    // 中文整句查询是这里最要紧的一档：`terms()` 按空格和逗号切词，一句没有
    // 空格的中文会被切成一个几十字的巨大 token，`ci_contains` 拿它去字面匹配
    // 必然全灭（实测 10 句人话里 7 句 RECALL_MISS）。BM25 的 2-gram 分词
    // 正好补这个洞，这也是它必须进来的第一理由。
    let mut rank_lists: Vec<Vec<String>> = Vec::new();

    // 路径 A：词面（保留，精确查询要保证可比）
    let mut lex: Vec<(String, i32)> = Vec::new();
    for (lib, rec) in &pool {
        if let Some((score, _, _)) = score_record(rec, &qs) {
            lex.push((rel(root, &rec.path), score));
        }
    }
    lex.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    rank_lists.push(lex.iter().map(|(p, _)| p.clone()).collect());

    // 路径 B：BM25（走落盘索引；没有索引时回退到现建）
    //
    // 这一步以前每次都 `semantic::Index::build`，857 条要 90ms、两万条更久。
    // 现在先试 `.index/`：命中就直接查表（不碰磁盘、不解析记录），
    // 索引缺失或与磁盘对不上时才回退重建。
    //
    // 回退路径必须保留：`rm -rf .index/` 之后一切照常，是索引设计里
    // "永远可弃"那条承诺的实现。
    let bm_paths: Vec<String> = match crate::idx::Index::load(root) {
        Some(disk) => {
            let hits = disk.search(q, pool.len().max(1));
            hits.iter()
                .filter_map(|(i, _, _)| disk.path_of(*i).map(|s| s.to_string()))
                .collect()
        }
        None => {
            let mem = crate::semantic::Index::build(root, libs);
            let bm = mem.query_depth(q, 1, pool.len().max(1));
            bm.iter()
                .filter_map(|(i, _, _)| mem.docs.get(*i).map(|d| d.path.clone()))
                .collect()
        }
    };
    rank_lists.push(bm_paths);

    // 路径 C：n-gram 片段（从"兜底"改成"常驻加分"）
    let mut ng: Vec<(String, i32)> = Vec::new();
    for (_, rec) in &pool {
        if let Some((score, _, _)) = score_ngram(rec, &qs) {
            ng.push((rel(root, &rec.path), score));
        }
    }
    ng.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    rank_lists.push(ng.iter().map(|(p, _)| p.clone()).collect());

    if std::env::var("ABOOK_DEBUG_TERMS").is_ok() {
        for (i, l) in rank_lists.iter().enumerate() {
            eprintln!("[PATH{i}] 命中 {} 条，前3: {:?}", l.len(), l.iter().take(3).collect::<Vec<_>>());
        }
    }
    // RRF 合并：score = Σ 1/(k + rank)，k 取 60 是文献常用值
    let k_rrf = 60.0f64;
    let mut rrf: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for list in &rank_lists {
        for (rank, path) in list.iter().enumerate() {
            *rrf.entry(path.clone()).or_insert(0.0) += 1.0 / (k_rrf + (rank + 1) as f64);
        }
    }

    // 只保留至少被一路命中的记录，按 RRF 分排序
    let mut order: Vec<(String, f64)> = rrf.into_iter().collect();
    order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));

    let by_path: std::collections::HashMap<String, (Lib, Record)> = pool
        .iter()
        .map(|(l, r)| (rel(root, &r.path), (*l, r.clone())))
        .collect();

    // ---- 三级权重：人类交代的排最高 ----
    //
    // 库里本来就有 KIND / MATURITY / AUTHORITY 三个字段，但召回评分一眼都没看它们。
    // 结果 178 条「用户明确交代」和 160 条「模型自己推的」得到完全相同的分数——
    // 你说过的和你没说的，在工具眼里没有区别。这就是"我让他记的他好像没记住"
    // 的机制层原因：不是没记，是记了之后没有任何东西让它优先出现。
    //
    // 分层判据直接落在现成字段上，不新增字段、不迁移旧数据：
    //   Tier 1（备忘录）：AUTHORITY = USER_EXPLICIT / USER_FEEDBACK
    //   Tier 3（经历）  ：KIND ∈ {RESULT,EVIDENCE,FAILURE,ATTEMPT} 或 AUTHORITY = TOOL_EVIDENCE
    //   Tier 2（经验）  ：其余
    //
    // 但 Tier 1 不能无脑置顶。它只是"你说过"，不代表"和这次相关"。所以设一个
    // 相关性准入线：RRF 分低于阈值的 Tier 1 不参与——宁可不出，也不能拿"你交代过"
    // 顶掉真正相关的记录。那会让检索变成"永远只返回那 178 条"。
    let tier_of = |rec: &Record| -> u8 {
        let auth = rec.get("AUTHORITY").trim().to_uppercase();
        let kind = rec.get("KIND").trim().to_uppercase();
        if auth == "USER_EXPLICIT" || auth == "USER_FEEDBACK" {
            return 1;
        }
        if matches!(kind.as_str(), "RESULT" | "EVIDENCE" | "FAILURE" | "ATTEMPT")
            || auth == "TOOL_EVIDENCE"
        {
            return 3;
        }
        2
    };
    let maturity_factor = |rec: &Record| -> f64 {
        match rec.get("MATURITY").trim().to_uppercase().as_str() {
            "VERIFIED" => 1.15,
            "TESTED" => 1.05,
            "OBSERVED" => 1.00,
            "PARTIAL" => 0.90,
            "HYPOTHESIS" => 0.85,
            "DISPROVED" => 0.10,
            _ => 1.00,
        }
    };
    // 系数是实测调出来的，不是拍的：
    //   1.45 时 Recall@5 从 4/10 掉到 3/10——178 条 Tier 1 一起涌上来，
    //   把真正相关的 Tier 2 挤下去了。这说明"人类交代过"这个信号的强度，
    //   不足以支撑那么大的加权：它应该让相关的更靠前，不该让不相关的也靠前。
    //   1.12 是"能推动排序但不至于翻盘"的量级。
    const TIER_FACTOR: [f64; 4] = [1.0, 1.12, 1.00, 0.95];
    // Tier 1 准入线：低于这个 RRF 分说明它只是"你说过"而并不相关
    // 准入线按 RRF 分位设：三路各命中一次约 3*(1/61)=0.049，单路命中约 0.016。
    // 取 0.020 意味着"至少两路都看到了它"，才配得上 Tier 1 的加权。
    const TIER1_MIN_RRF: f64 = 0.020;

    for (path, rrf_score) in order.iter_mut() {
        if let Some((_, rec)) = by_path.get(path) {
            let t = tier_of(rec);
            let mut f = TIER_FACTOR[t as usize] * maturity_factor(rec);
            if t == 1 && *rrf_score < TIER1_MIN_RRF {
                // 不相关的人类交代不该挤掉相关内容：降回中性，而不是置顶
                f = 0.90 * maturity_factor(rec);
            }
            *rrf_score *= f;
        }
    }
    order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));

    for (path, rrf_score) in &order {
        let (lib, rec) = match by_path.get(path) {
            Some(v) => v,
            None => continue,
        };
        let l = *lib;
        let scored = Some(((*rrf_score * 1000.0) as i32, Tier::Lexical, "rrf".to_string()));
        if let Some((score, tier, reason)) = scored {
            let path = rel(root, &rec.path);
            // Only hits that survived scoring pay for a directory listing.
            let att = crate::attach::scan(&rec.path);
            let rej = precision(rec, &qs);
            let mut r2 = rec.clone();
            if !with_body {
                r2.body = String::new();
            }
            let h = Hit {
                lib: *lib,
                path,
                title: rec.title(),
                tier,
                score,
                reason,
                rejected: rej.clone(),
                record: Some(r2),
                updated: rec.updated(),
                bytes: rec.raw.len(),
                lines: rec.raw.lines().count(),
                attach_count: att.count(),
                attach_bytes: att.bytes,
            };
            if rej.is_some() {
                res.rejected.push(h);
            } else {
                hits.push(h);
            }
        }
    }

    // branch-summary tier: only if nothing lexical matched
    if hits.is_empty() {
        for lib in libs {
            if let Ok(a) = Atlas::load(&root.join(lib.atlas())) {
                for b in &a.branches {
                    let mut sc = 0;
                    let mut why = Vec::new();
                    for q in &qs {
                        if ci_contains(&b.get("KEYS"), q) || ci_contains(&b.name(), q) {
                            sc += 30;
                            why.push("branch");
                        }
                        if ci_contains(&b.get("READ_WHEN"), q) {
                            sc += 15;
                            why.push("branch-read-when");
                        }
                        if ci_contains(&b.get("SUMMARY"), q) {
                            sc += 8;
                            why.push("branch-summary");
                        }
                    }
                    if sc > 0 {
                        for p in b.paths() {
                            let full = root.join(&p);
                            if !full.exists() {
                                continue;
                            }
                            if let Ok(rec) = Record::parse(&full, *lib) {
                                let att = crate::attach::scan(&rec.path);
                                let mut r2 = rec.clone();
                                if !with_body {
                                    r2.body = String::new();
                                }
                                hits.push(Hit {
                                    lib: *lib,
                                    path: p.clone(),
                                    title: rec.title(),
                                    tier: Tier::Branch,
                                    score: sc,
                                    reason: why.join("+"),
                                    rejected: None,
                                    record: Some(r2),
                                    updated: rec.updated(),
                                    bytes: rec.raw.len(),
                                    lines: rec.raw.lines().count(),
                                    attach_count: att.count(),
                                    attach_bytes: att.bytes,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.path.cmp(&b.path)));
    hits.truncate(limit);
    res.tiers_used = hits.iter().map(|h| h.tier).collect();
    res.tiers_used.sort_by_key(|t| *t as u8);
    res.tiers_used.dedup();
    res.status = if !hits.is_empty() {
        "RECALL_OK"
    } else if !res.rejected.is_empty() {
        "RECALL_UNRESOLVED"
    } else {
        "RECALL_MISS"
    };
    res.hits = hits;
    res
}

/// Aggregate state read from Global.md.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub root: String,
    pub canonical: usize,
    pub candidate: usize,
    pub canonical_atlas_count: usize,
    pub candidate_atlas_count: usize,
    pub covered_canonical: String,
    pub covered_candidate: String,
    pub latest_commit: String,
    pub vault_files: usize,
    pub vault_rows: usize,
    pub problems: Vec<String>,
}

/// 统计某库中非 DELETED 的记录数，只读头部、不解析记录。
///
/// 与 `load_lib(...).iter().filter(|r| r.status() != "DELETED").count()`
/// 的结果相同，但不构造 `Record`、不解析字段表、不克隆正文。
///
/// 判 DELETED 只看头部里 `STATUS=` 那一行：格式约定状态写在头部，正文里
/// 出现同名文本不算。读不全或读不出时按"非删除"计入——宁可多算一条，
/// 也不要把一条活记录当成删掉的（那会让 `state` 显示的数字比磁盘少，
/// 反而误导读库的人）。
/// 头部里是否写着 `STATUS=DELETED`。
///
/// 单独抽出来是因为它有两个容易踩的坑，两处都会把"已删"读成"还在"：
///
/// 1. **4KB 窗口按字节截断**。头部里有大量中文（每字 3 字节），一条字段多的
///    记录，`STATUS` 行的字节位置可能已经越过 4096。实测候选库里正好有 5 条
///    这样的记录：全部软删之后 `state` 报"目录 5 条 vs 地图 0 条"，一致性失败，
///    而 `list --status DELETED` 同时报 301 条——两个命令对同一份数据给出不同
///    答案。所以窗口里找不到时必须回退读全文，不能当作"没有这行"。
/// 2. **字段可能被压进一行**。`ID=x;PROJECT=y;STATUS=DELETED` 是合法写法，
///    只比对整行会漏。
fn head_says_deleted(p: &Path, head: &str) -> bool {
    let hit = |text: &str| {
        text.lines().any(|l| {
            l.trim()
                .split(';')
                .any(|part| part.trim() == "STATUS=DELETED" || part.trim() == "STATUS=del")
        })
    };
    if hit(head) {
        return true;
    }
    // 窗口里没有：可能是被截断了。读全文再判一次。
    // 只在"4KB 里根本没有 STATUS 行"时才会走到这里，所以绝大多数记录不付
    // 这份 I/O —— 而没有这一步，那几条长头部的记录会被永久算成"还在"。
    let has_status_line = head.lines().any(|l| {
        l.trim().split(';').any(|part| part.starts_with("STATUS="))
    });
    if has_status_line {
        return false;
    }
    match crate::util::read_str(p) {
        Ok(full) => hit(&full),
        // 读不出来：不猜。当作"还在"（保守），但这种情况已经在上面的 Err 分支
        // 里被计为一条记录，不在这里重复放大。
        Err(_) => false,
    }
}

fn count_live_fast(root: &Path, lib: Lib) -> usize {
    let dir = root.join(lib.dir());
    let rd = match std::fs::read_dir(&dir) {
        Ok(x) => x,
        Err(_) => return 0,
    };
    let mut n = 0usize;
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().map(|x| x != "md").unwrap_or(true) {
            continue;
        }
        // 只读前 4KB：头部字段全在最前面，正文再长也不用读。
        let head = match crate::util::read_head(&p, 4096) {
            Ok(x) => x,
            Err(_) => {
                n += 1;
                continue;
            }
        };
        if !head_says_deleted(&p, &head) {
            n += 1;
        }
    }
    n
}

pub fn read_state(root: &Path) -> State {
    let mut st = State {
        root: root.to_string_lossy().to_string(),
        ..Default::default()
    };
    // Count only records in the current view: a soft-deleted record is still
    // on disk (so it can be restored) but is no longer part of the library.
    //
    // 数个数不该把每条记录完整解析一遍。
    //
    // 原来这里走 load_lib，对 972 个文件逐个 fs::read + Record::parse
    // （实测 1010 次 openat、6740 次 statx），只为了得到两个整数。单位成本
    // 约 0.08 ms/条，`state` 因此 87 ms——而它是最常被调用的命令：每次开工
    // 看库状态都要付这笔钱。1% 的库增长就直接变成 1 秒。
    //
    // 只要 STATUS 字段，就只读 STATUS 字段。判断"这条是不是 DELETED"不需要
    // 解析正文、不需要建字段表、不需要克隆任何东西——扫一遍头部文本即可。
    // 语义完全一致：DELETED 一定写在头部，这是 reb::Record 的格式约定。
    let live = |lib: Lib| -> usize { count_live_fast(root, lib) };
    st.canonical = live(Lib::Canonical);
    st.candidate = live(Lib::Candidate);
    if let Ok(a) = Atlas::load(&root.join(Lib::Canonical.atlas())) {
        st.canonical_atlas_count = a.total_count();
        st.covered_canonical = a.get("COVERED_THROUGH");
    }
    if let Ok(a) = Atlas::load(&root.join(Lib::Candidate.atlas())) {
        st.candidate_atlas_count = a.total_count();
        st.covered_candidate = a.get("COVERED_THROUGH");
    }
    if let Ok(g) = crate::book::BookFile::load(&root.join("Global.md")) {
        st.latest_commit = g.state("LATEST_COMMIT").unwrap_or_default();
    }
    let (f, r, p) = crate::vault::reconcile(root);
    st.vault_files = f;
    st.vault_rows = r;
    st.problems.extend(p);
    if st.canonical != st.canonical_atlas_count {
        st.problems.push(format!(
            "正式库目录 {} 条 vs 地图 {} 条",
            st.canonical, st.canonical_atlas_count
        ));
    }
    if st.candidate != st.candidate_atlas_count {
        st.problems.push(format!(
            "候选库目录 {} 条 vs 地图 {} 条",
            st.candidate, st.candidate_atlas_count
        ));
    }
    st
}

/// Choose a branch for a new record from its fields.
pub fn guess_branch(rec: &Record) -> String {
    let kind = rec.kind();
    let title = rec.title();
    let text = format!("{title} {}", rec.get("KEYS"));
    if ci_contains(&text, "凭据") || ci_contains(&text, "保险箱") || ci_contains(&text, "vault") {
        return "凭据保险箱".into();
    }
    match kind.as_str() {
        "FAILURE" | "ATTEMPT" => "失败与回滚".into(),
        "CORRECTION" => "失败与回滚".into(),
        "EVIDENCE" => "审计证据".into(),
        "DECISION" => "记忆维护与演进".into(),
        _ => DEFAULT_BRANCH.into(),
    }
}

#[cfg(test)]
mod live_count_tests {
    use super::*;

    /// 头部窗口里找不到 `STATUS=DELETED` 时，必须区分"确实没有"与"被截断了"。
    ///
    /// 真实故障：五条候选的头部字节数超过 4096（中文字段名与长正文摘要），
    /// 全部软删之后 `state` 报"候选库目录 5 条 vs 地图 0 条"，一致性失败；
    /// 同一时刻 `list --status DELETED` 报 301 条。两个命令对同一份数据给出
    /// 不同答案，根因就是这个 4KB 窗口按字节截断。
    #[test]
    fn truncated_head_must_fall_back_to_full_read() {
        let dir = std::env::temp_dir().join(format!("abook-livecount-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 短头部：窗口内就能判定。
        let short = dir.join("short.md");
        std::fs::write(&short, "ID=A\nSTATUS=DELETED\n").unwrap();
        let head = crate::util::read_head(&short, 4096).unwrap();
        assert!(head_says_deleted(&short, &head));

        // 长头部：STATUS 落在 4096 之外。旧实现会答"没删"。
        let long = dir.join("long.md");
        let filler = "备注=".to_string() + &"甲".repeat(2000);
        std::fs::write(&long, format!("ID=B\n{filler}\nSTATUS=DELETED\n")).unwrap();
        let head = crate::util::read_head(&long, 4096).unwrap();
        assert!(
            !head.contains("STATUS=DELETED"),
            "前提：窗口必须真的截掉了那一行"
        );
        assert!(
            head_says_deleted(&long, &head),
            "窗口截断时必须回退读全文，否则已删记录被算成还在"
        );

        // 打包在一行里的字段也要认：`ID=x;STATUS=DELETED` 是合法写法。
        let packed = dir.join("packed.md");
        std::fs::write(&packed, "ID=C;PROJECT=p;STATUS=DELETED\n").unwrap();
        let head = crate::util::read_head(&packed, 4096).unwrap();
        assert!(head_says_deleted(&packed, &head));

        // 没删的不能被误判成删了。
        let live = dir.join("live.md");
        std::fs::write(&live, "ID=D\nSTATUS=ACTIVE\n").unwrap();
        let head = crate::util::read_head(&live, 4096).unwrap();
        assert!(!head_says_deleted(&live, &head));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 纯索引召回：只在索引上打分排序，最后才读命中的那几条正文。
///
/// 这是 O(1) 的来源。老路径要先把全库记录读进内存（`load_lib`），
/// 两万条时 1.4 秒；这条路只碰 `.index/` 两个文件，与库里有几条记录无关。
fn recall_via_index(
    root: &Path,
    q: &str,
    qs: &[String],
    disk: &crate::idx::Index,
    limit: usize,
    with_body: bool,
) -> RecallResult {
    let mut res = RecallResult {
        query: q.to_string(),
        status: "RECALL_MISS",
        scanned: disk.total_docs,
        ..Default::default()
    };

    // BM25 只在 postings 上算，不碰 docs。这一步是常数级的关键：
    // postings 体积取决于词表，与记录数关系弱。
    let bm = disk.search_shallow(q, limit.saturating_mul(20).max(200));

    // 把命中的 id 按分片归组，每个分片只读一次。
    // 不逐条读：那样 N 条命中就是 N 次 open+read，比分片读贵得多。
    // 候选集 = BM25 命中 ∪ 词面命中。
    //
    // 这里以前只有 BM25 命中，于是词面与 n-gram 两路都只在 BM25 的结果里挑，
    // 等于"第一路决定候选集，其余两路陪跑"。实测后果：查询 `abook并发写入`
    // 时 BM25 分词把它切碎、没命中那条正确记录，而记录标题里明明就写着
    // "abook 并发写入实测"——词面本可以兜住，却没机会看到它。
    //
    // 词面候选从**索引里已加载的 Doc 元信息**（标题与 KEYS）里筛，
    // 只读 docs 分片、不解析记录正文，仍然是常数级。
    let mut need: std::collections::BTreeMap<usize, Vec<usize>> = std::collections::BTreeMap::new();
    let mut cand: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (id, _, _) in &bm {
        if cand.insert(*id) {
            need.entry(disk.shard_of_id(*id)).or_default().push(*id);
        }
    }
    // 词面补充：**走 head_postings，不扫 docs，也不用全量 postings**。
    //
    // 这里曾经是 `for (id, d) in disk.docs.iter()`——遍历全量 Doc 做子串匹配。
    // 它是对的但代价 O(n)，而 build 早已把 title 与 KEYS 分词写进 postings，
    // 所以按词查桶即可拿到同一批候选，代价只与词频有关。
    //
    // 精度上这里只做"是否同现"，不排序；排序交给后面的 RRF 在真实 doc 上算。
    //
    // 注意这里用 `tokenize` 而不是 `qs`：postings 是按汉字二元组建的键，
    // 而 `terms` 把连续中文保留成整段（`abook并发写入实测` → ["abook",
    // "并发写入实测"]）。拿整段去查 postings 必然查不到——词组在库里存的是
    // 「并发/发写/写入/…」。两条切词规则各用在对的地方：
    //   `terms`    → 记录正文子串匹配（旧路径）
    //   `tokenize` → head_postings 键查找（索引路径）
    let qterms: Vec<String> = crate::semantic::tokenize(q);
    for t in &qterms {
        if let Some(plist) = disk.head_postings.get(t) {
            for (id, _) in plist {
                let id = *id as usize;
                if cand.insert(id) {
                    need.entry(disk.shard_of_id(id)).or_default().push(id);
                }
            }
        }
    }
    if std::env::var("ABOOK_DEBUG_RECALL").is_ok() {
        eprintln!("[RECALL] q={q:?} tokenize={:?}", crate::semantic::tokenize(q));
        eprintln!("[RECALL] bm 候选 {} 条，词面后候选 {} 条", bm.len(), cand.len());
        eprintln!("[RECALL] 528 在候选集: {}", cand.contains(&528));
    }
    let mut docs_by_id: std::collections::HashMap<usize, crate::idx::Doc> =
        std::collections::HashMap::new();
    for sh in need.keys() {
        for (id, d) in disk.shard(root, *sh) {
            docs_by_id.insert(id, d);
        }
    }

    // RRF：BM25 排名 + 词面（在已加载的命中集合上做）+ n-gram
    let mut rank_lists: Vec<Vec<String>> = Vec::new();

    // BM25 排名
    rank_lists.push(
        bm.iter()
            .filter_map(|(i, _, _)| docs_by_id.get(i).map(|d| d.path.clone()))
            .collect(),
    );

    // 词面（只在候选集合上做，不扫全库）
    let mut lex: Vec<(String, i32)> = Vec::new();
    for (_, d) in docs_by_id.iter() {
        let mut sc = 0i32;
        // 这里必须遍历 `qterms`（tokenize 的二元组），不能用 `qs`。
        //
        // `qs` 是 `terms()` 的结果，中文连续段被整段保留：`索引放哪里` →
        // `["索引放哪里"]`。拿它去做 `title.contains()`，等于要求标题里出现
        // 这五个字的完整连续串，实际永远为假——标题这一路打了很久的 0 分，
        // 却没人发现，因为"没有加分"和"没有命中"在输出上看不出区别。
        //
        // 实测后果：查「索引放哪里」时，标题正是《记忆资产身份与索引维护》
        // 的那条记录词面得 0，被一条正文反复出现"索引"的长记录挤掉。
        for t in &qterms {
            if ci_contains(&d.title, t) {
                sc += 60;
            }
            if d.keys.iter().any(|k| ci_contains(k, t)) {
                sc += 100;
            }
        }
        if sc > 0 {
            lex.push((d.path.clone(), sc));
        }
    }
    lex.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    rank_lists.push(lex.iter().map(|(p, _)| p.clone()).collect());

    // n-gram（标题 + postings 词存在性）
    let mut ng: Vec<(String, i32)> = Vec::new();
    for (_, d) in docs_by_id.iter() {
        let mut sc = 0i32;
        for t in qs {
            for g in ngrams(t) {
                if ci_contains(&d.title, &g) {
                    sc += 15;
                } else if disk.has_term_in(&g, &d.path) {
                    sc += 8;
                }
            }
        }
        if sc > 0 {
            ng.push((d.path.clone(), sc));
        }
    }
    ng.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    rank_lists.push(ng.iter().map(|(p, _)| p.clone()).collect());

    let k_rrf = 20.0f64;
    const W: [f64; 3] = [0.5, 2.2, 0.5];
    let mut rrf: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for (li, list) in rank_lists.iter().enumerate() {
        for (rank, path) in list.iter().enumerate() {
            *rrf.entry(path.clone()).or_insert(0.0) += W[li] / (k_rrf + (rank + 1) as f64);
        }
    }

    let by_path: std::collections::HashMap<&str, &crate::idx::Doc> =
        docs_by_id.values().map(|d| (d.path.as_str(), d)).collect();

    let mut order: Vec<(String, f64)> = rrf.into_iter().collect();
    for (path, sc) in order.iter_mut() {
        if let Some(d) = by_path.get(path.as_str()) {
            let a = d.authority.trim().to_uppercase();
            let k = d.kind.trim().to_uppercase();
            let tier = if a == "USER_EXPLICIT" || a == "USER_FEEDBACK" {
                1
            } else if matches!(k.as_str(), "RESULT" | "EVIDENCE" | "FAILURE" | "ATTEMPT")
                || a == "TOOL_EVIDENCE"
            {
                3
            } else {
                2
            };
            let base = match tier {
                1 => 1.12,
                3 => 0.95,
                _ => 1.00,
            };
            let mf = match d.maturity.trim().to_uppercase().as_str() {
                "VERIFIED" => 1.15,
                "TESTED" => 1.05,
                "OBSERVED" => 1.00,
                "PARTIAL" => 0.90,
                "HYPOTHESIS" => 0.85,
                "DISPROVED" => 0.10,
                _ => 1.00,
            };
            let f = if tier == 1 && *sc < 0.020 { 0.90 } else { base };
            *sc *= f * mf;
        }
    }
    order.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    order.truncate(limit);

    for (path, sc) in &order {
        let full = root.join(path);
        let lib = if path.starts_with("candidate/") {
            Lib::Candidate
        } else {
            Lib::Canonical
        };
        let rec = match Record::parse(&full, lib) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if rec.status() == "DELETED" {
            continue;
        }
        let att = crate::attach::scan(&rec.path);
        let rej = precision(&rec, qs);
        let mut r2 = rec.clone();
        if !with_body {
            r2.body = String::new();
        }
        let h = Hit {
            lib,
            path: path.clone(),
            title: rec.title(),
            tier: Tier::Lexical,
            score: (*sc * 1000.0) as i32,
            reason: "rrf/index".to_string(),
            rejected: rej.clone(),
            record: Some(r2),
            updated: rec.updated(),
            bytes: rec.raw.len(),
            lines: rec.raw.lines().count(),
            attach_count: att.count(),
            attach_bytes: att.bytes,
        };
        if rej.is_some() {
            res.rejected.push(h);
        } else {
            res.hits.push(h);
        }
    }

    res.status = if !res.hits.is_empty() {
        "RECALL_OK"
    } else if !res.rejected.is_empty() {
        "RECALL_UNRESOLVED"
    } else {
        "RECALL_MISS"
    };
    res
}
