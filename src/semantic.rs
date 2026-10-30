//! Semantic-ish retrieval over the memory book.
//!
//! A real embedding model would need weights shipped alongside the binary,
//! which contradicts "one file, no side artefacts". So this module gets its
//! semantics from the corpus itself:
//!
//!   * tokenisation that works for Chinese without a dictionary: CJK runs
//!     become overlapping 2-grams, Latin runs become lowercase words;
//!   * BM25 ranking with an in-memory inverted index (memory scales with
//!     vocabulary, not with corpus size — bodies are never retained);
//!   * query expansion by association: the top BM25 hits contribute terms
//!     from their own KEYS/CONCEPTS fields, and the second pass searches for
//!     those too (Rocchio-style pseudo-relevance feedback).
//!
//! That last step is what makes it semantic in practice: a query phrased in
//! one vocabulary reaches records written in another, because the library's
//! own recall keys act as the thesaurus.
//!
//! Nothing is written to disk. Every search rebuilds in memory, which for a
//! few thousand records costs far less than reading them from page cache.

use crate::rec::{Lib, Record};
use crate::util::*;
use std::collections::HashMap;
use std::path::Path;

/// Characters that carry no retrieval signal on their own. A bigram made only
/// of these (or containing one as a boundary crossing) is noise: this is where
/// artefacts like "默失" came from.
const STOP_CJK: &[char] = &[
    '的', '了', '是', '在', '和', '与', '或', '也', '就', '都', '而', '及', '其', '之',
    '这', '那', '上', '下', '中', '为', '以', '对', '从', '到', '被', '把', '让', '给',
    '不', '没', '很', '会', '要', '能', '可', '有', '无', '个', '些', '么', '什', '怎',
    '我', '你', '他', '她', '它', '们', '一', '二', '三', '两', '请', '吗', '呢', '吧',
    '着', '过', '得', '地', '如', '若', '则', '并', '但', '还', '又', '再', '更',
    // 疑问与情态虚词。中文口语提问几乎句句带它们，而它们对"这条记录讲什么"
    // 零贡献。不滤掉的话，2-gram 会把它们和实词拼成有效词（"应该在""该放哪"），
    // 命中一堆只是恰好含有这些字的记录，把真正相关的挤下去。
    '应', '该', '怎', '么', '哪', '些', '谁', '何',
    '要', '想', '需', '须', '用', '做', '搞', '弄', '看', '说', '问',
];

fn is_stop(c: char) -> bool {
    STOP_CJK.contains(&c)
}

/// Tokenise into index terms. CJK: overlapping bigrams. Latin: words.
/// Single CJK chars are emitted too, at lower weight, so a one-character
/// query still matches.
pub fn tokenize(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let cjk = |c: char| (c as u32) >= 0x3400;
    let mut i = 0;
    while i < chars.len() {
        if cjk(chars[i]) {
            let start = i;
            while i < chars.len() && cjk(chars[i]) {
                i += 1;
            }
            let run = &chars[start..i];
            if run.len() == 1 {
                if !is_stop(run[0]) {
                    out.push(run[0].to_string());
                }
            } else {
                for w in run.windows(2) {
                    // A bigram that brackets a stop character is a word
                    // boundary, not a term.
                    if is_stop(w[0]) || is_stop(w[1]) {
                        continue;
                    }
                    out.push(w.iter().collect());
                }
            }
        } else if chars[i].is_alphanumeric() {
            let start = i;
            // 关键：`is_alphanumeric()` 对汉字也返回 true，所以这里必须显式
            // 排除 CJK，否则 `abook并发写入实测` 会被当成**一个** 11 字 token，
            // 与库里任何二元组都对不上——带空格的同义查询却能命中，因为空格
            // 恰好断开了两者。这一步是"中英连写查询全灭"的真正根因。
            while i < chars.len()
                && ((chars[i].is_alphanumeric() && !cjk(chars[i]))
                    || chars[i] == '_'
                    || chars[i] == '-')
            {
                i += 1;
            }
            let w: String = chars[start..i].iter().collect::<String>().to_lowercase();
            if w.chars().count() >= 2 {
                out.push(w.clone());
                // Identifiers dominate this library's vocabulary:
                // `CANDIDATE_WRITE_SILENT_GATE`, `AB2V1`, `canonical/records`.
                // Treated as one opaque word they can only be found by their
                // full spelling, so a search for `GATE` returned 1 record while
                // a literal scan found 169. Split on `_` and `-` and index the
                // parts alongside the whole, which keeps the exact spelling
                // searchable while making the components reachable.
                for part in w.split(['_', '-']) {
                    if part.chars().count() >= 2 && part != w.as_str() {
                        out.push(part.to_string());
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct DocMeta {
    pub path: String,
    pub lib: Lib,
    pub title: String,
    pub len: u32,
    /// Curated terms taken from KEYS/CONCEPTS (comma-separated, already words).
    /// Expansion draws from these rather than from bigrams, because a bigram
    /// built over prose crosses word boundaries and yields things like "忆开".
    pub keys: Vec<String>,
}

/// Split a curated field into terms. These are author-written words, so they
/// must NOT go through the bigram tokeniser.
fn curated_terms(v: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in v.split([',', '，', ';', '、', '|']) {
        let t = raw.trim().to_lowercase();
        if t.chars().count() < 2 {
            continue;
        }
        if !out.contains(&t) {
            out.push(t.clone());
        }
        // Curated keys are overwhelmingly `SOMETHING_GATE` identifiers. Expansion
        // votes on these terms, so a key stored only as one long token can never
        // contribute its own component words back to a later query.
        for part in t.split(['_', '-']) {
            let sub = part.to_string();
            if sub.chars().count() >= 2 && sub != t && !out.contains(&sub) {
                out.push(sub);
            }
        }
    }
    out
}

pub struct Index {
    postings: HashMap<String, Vec<(u32, u32)>>,
    pub docs: Vec<DocMeta>,
    avgdl: f64,
    pub total_terms: usize,
}

impl Index {
    /// Build in memory. Bodies are tokenised then dropped.
    ///
    /// Parsing and tokenising dominate the cost, so the file list is sharded
    /// across cores exactly like the literal scanner does. Merging afterwards
    /// is sequential but cheap: it only moves postings into the shared map.
    pub fn build(root: &Path, libs: &[Lib]) -> Index {
        struct FileJob {
            path: std::path::PathBuf,
            rel: String,
            lib: Lib,
        }
        let mut files: Vec<FileJob> = Vec::new();
        for &lib in libs {
            let dir = root.join(lib.dir());
            let rd = match std::fs::read_dir(&dir) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let mut paths: Vec<std::path::PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().map(|x| x == "md").unwrap_or(false))
                .collect();
            paths.sort();
            for p in paths {
                files.push(FileJob {
                    rel: rel(root, &p),
                    path: p,
                    lib,
                });
            }
        }

        struct Parsed {
            rel: String,
            lib: Lib,
            title: String,
            terms: Vec<String>,
            keys: Vec<String>,
        }

        let nthreads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .max(1)
            .min(files.len().max(1));
        let chunk = (files.len() + nthreads - 1) / nthreads.max(1);

        let mut parsed: Vec<Parsed> = Vec::new();
        std::thread::scope(|sc| {
            let mut handles = Vec::new();
            for part in files.chunks(chunk.max(1)) {
                handles.push(sc.spawn(move || {
                    let mut local: Vec<Parsed> = Vec::new();
                    for f in part {
                        let rec = match Record::parse(&f.path, f.lib) {
                            Ok(r) => r,
                            Err(_) => continue,
                        };
                        if rec.status() == "DELETED" {
                            continue;
                        }
                        let mut keys = curated_terms(&rec.get("KEYS"));
                        for t in curated_terms(&rec.get("CONCEPTS")) {
                            if !keys.iter().any(|k| *k == t) {
                                keys.push(t);
                            }
                        }
                        let mut buf = String::new();
                        buf.push_str(&rec.title());
                        buf.push(' ');
                        buf.push_str(&rec.get("KEYS"));
                        buf.push(' ');
                        buf.push_str(&rec.get("CONCEPTS"));
                        buf.push(' ');
                        buf.push_str(&rec.get("ENTITIES"));
                        buf.push(' ');
                        buf.push_str(&rec.get("READ_WHEN"));
                        buf.push(' ');
                        buf.push_str(&rec.get("SUMMARY"));
                        buf.push(' ');
                        buf.push_str(&rec.body);
                        let mut terms = tokenize(&buf);
                        for k in &keys {
                            terms.push(k.clone());
                            if k.chars().count() > 2 {
                                terms.push(k.clone());
                            }
                        }
                        local.push(Parsed {
                            rel: f.rel.clone(),
                            lib: f.lib,
                            title: rec.title(),
                            terms,
                            keys,
                        });
                    }
                    local
                }));
            }
            for h in handles {
                if let Ok(v) = h.join() {
                    parsed.extend(v);
                }
            }
        });

        let mut docs: Vec<DocMeta> = Vec::new();
        let mut postings: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        let mut total_len = 0u64;
        for p in parsed {
            let dl = p.terms.len() as u32;
            if dl == 0 {
                continue;
            }
            let id = docs.len() as u32;
            let mut tf: HashMap<String, u32> = HashMap::new();
            for t in p.terms {
                *tf.entry(t).or_insert(0) += 1;
            }
            for (t, c) in tf {
                postings.entry(t).or_default().push((id, c));
            }
            total_len += dl as u64;
            docs.push(DocMeta {
                path: p.rel,
                lib: p.lib,
                title: p.title,
                len: dl,
                keys: p.keys,
            });
        }
        let avgdl = if docs.is_empty() {
            1.0
        } else {
            total_len as f64 / docs.len() as f64
        };
        let total_terms = postings.len();
        Index {
            postings,
            docs,
            avgdl,
            total_terms,
        }
    }

    fn idf(&self, term: &str) -> f64 {
        let n = self.docs.len() as f64;
        let df = self.postings.get(term).map(|v| v.len()).unwrap_or(0) as f64;
        // BM25 idf with the +0.5 smoothing, floored so unseen terms are not
        // negative-scored into oblivion.
        let v = ((n - df + 0.5) / (df + 0.5)).ln();
        v.max(0.0)
    }

    /// One BM25 pass. `weights` lets the caller bias some query terms.
    fn bm25(&self, terms: &[(String, f64)]) -> Vec<f64> {
        let mut scores = vec![0.0f64; self.docs.len()];
        let k1 = 1.4f64;
        let b = 0.72f64;
        for (t, w) in terms {
            let idf = self.idf(t);
            if idf <= 0.0 {
                continue;
            }
            let plist = match self.postings.get(t) {
                Some(p) => p,
                None => continue,
            };
            for (id, tf) in plist {
                let dl = self.docs[*id as usize].len as f64;
                let denom = *tf as f64 + k1 * (1.0 - b + b * dl / self.avgdl);
                scores[*id as usize] += w * idf * (*tf as f64 * (k1 + 1.0)) / denom;
            }
        }
        scores
    }

    pub fn query_depth(&self, q: &str, depth: u8, k: usize) -> Vec<(usize, f64, String)> {
        let base: Vec<(String, f64)> = tokenize(q).into_iter().map(|t| (t, 1.0)).collect();
        if base.is_empty() {
            return Vec::new();
        }
        let mut scores = self.bm25(&base);
        let mut why: Vec<String> = vec![String::new(); self.docs.len()];

        // Coverage: a record matched only by one generic word is not a hit.
        // Requiring a share of the query's terms is what turns BM25 from
        // "contains a word" into "is about the question".
        let distinct: Vec<&String> = {
            let mut v: Vec<&String> = base.iter().map(|(t, _)| t).collect();
            v.sort();
            v.dedup();
            v
        };
        // 覆盖率门槛：一条记录至少要命中查询里这么多个词，才算"在讲这件事"。
        //
        // 这个比例调过两次，值得记下为什么是现在这个数：
        //   * 原值 0.34 是从英文查询的经验搬来的。中文整句里虚词多
        //     （应该/哪里/怎么/要不要），它们各自散落在不同记录里，凑不齐
        //     34%，于是一句完全说得通的话被判 0 命中——实测「索引应该放哪里」
        //     「写入的时候要不要更新索引」都是 RECALL_MISS，而每个词单独都能
        //     搜到东西。这不是检索不到，是门槛把人挡在门外。
        //   * 长查询里这个词数本来就多，要求覆盖三成等于要求"命中一半主题"，
        //     对口语提问过严。
        //
        // 现在改成平方根量级：4 词要求 2、9 词要求 3、16 词要求 4。仍能挡住
        // "只命中一个泛词"的记录（那类记录 RRF 里也排不上来），但不会把整句话
        // 直接判死。
        let need = if distinct.len() >= 4 {
            (distinct.len() as f64).sqrt().floor() as usize
        } else if distinct.len() >= 2 {
            1
        } else {
            1
        };
        let mut covered = vec![0usize; self.docs.len()];
        for (t, _) in &base {
            if let Some(pl) = self.postings.get(t) {
                for (id, _) in pl {
                    covered[*id as usize] += 1;
                }
            }
        }
        for (i, s) in scores.iter_mut().enumerate() {
            if covered[i] < need {
                *s = 0.0;
            } else {
                // reward breadth of coverage, not just depth on one term
                *s *= 1.0 + 0.25 * (covered[i] as f64 / distinct.len() as f64);
            }
        }

        for _round in 0..depth {
            // Pseudo-relevance feedback: the strongest hits vote for the
            // vocabulary their own recall keys use.
            let mut order: Vec<usize> = (0..self.docs.len()).collect();
            order.sort_by(|a, b| scores[*b].partial_cmp(&scores[*a]).unwrap_or(std::cmp::Ordering::Equal));
            let nseed = if depth >= 2 { 12 } else { 5 };
            let seeds = &order[..order.len().min(nseed)];
            let mut votes: HashMap<String, f64> = HashMap::new();
            for &d in seeds {
                if scores[d] <= 0.0 {
                    continue;
                }
                for t in &self.docs[d].keys {
                    if base.iter().any(|(b, _)| b == t) {
                        continue;
                    }
                    *votes.entry(t.clone()).or_insert(0.0) += scores[d];
                }
            }
            let mut voted: Vec<(String, f64)> = votes
                .into_iter()
                // a term present in most documents carries no discriminative
                // power; skipping it is what keeps expansion from drifting
                .filter(|(t, _)| {
                    let df = self.postings.get(t).map(|v| v.len()).unwrap_or(0);
                    df * 12 < self.docs.len().max(1)
                })
                .collect();
            voted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            voted.truncate(24);
            // Drop terms that merely re-spell a stronger one (e.g. "险箱"
            // when "保险箱" is already chosen).
            let strong: Vec<String> = voted.iter().map(|(t, _)| t.clone()).collect();
            voted.retain(|(t, _)| {
                !strong
                    .iter()
                    .any(|o| o != t && o.chars().count() > t.chars().count() && o.contains(t.as_str()))
            });
            if !voted.is_empty() {
                let top = voted[0].1.max(1e-9);
                // expansion contributes, but never more than the literal query
                let weighted: Vec<(String, f64)> = voted
                    .into_iter()
                    .map(|(t, v)| (t, 0.45 * v / top))
                    .collect();
                let add = self.bm25(&weighted);
                for (i, s) in add.iter().enumerate() {
                    if *s > 0.0 {
                        scores[i] += s * if _round == 0 { 1.0 } else { 0.5 };
                        if why[i].is_empty() {
                            let hit: Vec<String> =
                                weighted.iter().map(|(t, _)| t.clone()).collect();
                            why[i] = format!(
                                "关联词 {}",
                                hit.iter().take(3).cloned().collect::<Vec<_>>().join("/")
                            );
                        }
                    }
                }
            }
        }

        let mut rows: Vec<(usize, f64, String)> = scores
            .iter()
            .enumerate()
            .filter(|(i, s)| **s > 0.0 && self.docs[*i].title.len() > 0)
            .map(|(i, s)| (i, *s, why[i].clone()))
            .collect();
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        rows.truncate(if k == 0 { rows.len() } else { k });
        rows
    }
}
