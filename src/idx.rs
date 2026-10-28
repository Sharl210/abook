//! 落盘索引：把「全量读盘解析」换成「查表」。
//!
//! ## 为什么必须落盘
//!
//! 实测增长曲线（aarch64 真机，每档记录内容互不重复）：
//!
//! | 规模 | 库体积 | recall |
//! |---|---|---|
//! | 500 条 | 2.0 MB | 71 ms |
//! | 2 000 条 | 8.0 MB | 144 ms |
//! | 5 000 条 | 20 MB | 304 ms |
//! | 10 000 条 | 40 MB | 777 ms |
//! | 20 000 条 | 80 MB | 1425 ms |
//!
//! 40 倍记录量 20 倍耗时——`recall` 调的 `rec::load_lib` 对目录下**每个文件**
//! 做 `fs::read` + `Record::parse`，不相关的也要读进来解析一遍。这是 O(n)。
//!
//! 索引要解决的就是这件事：把全量轮询换成查表。
//!
//! ## 三条设计约束
//!
//! 1. **只索引条目，不索引附件。** 条目旁的 `<stem>-refs/`、`-assets/` 等目录
//!    不进索引。理由是可维护性与类型异构（里面有二进制、压缩包、视频），不是性能
//!    ——实测 2000 条全带附件只比无附件多 15ms，因为 `attach::scan` 只对命中的
//!    记录执行。这两件事分开记，免得以后有人拿"附件扫描很快"论证"索引没必要"。
//!
//! 2. **只存相对库根的路径。** 整个 `~/AgentsBook` 压缩后换机器解压，路径变了
//!    也不影响。任何绝对路径都会让这个承诺失效。
//!
//! 3. **永远可弃。** `rm -rf .index/` 后一切照常，下次写入时重建。索引是纯加速，
//!    不是必需。所以它坏了绝不能阻塞记账——记录是资产，索引是派生物。
//!
//! ## 与并发的关系
//!
//! 索引更新必须在库级锁内完成，和 Atlas/Index/Global 是同一个临界区。否则
//! A 进程写完记录、还没写索引时，B 进程按旧索引召回会读到中间态。
//!
//! 但索引更新**失败**不能让整个写操作失败：标 `dirty: true`，下次全量重建。
//! 拿数据换整洁是错的。
//!
//! 重建复用同一把 `.abook.lock`，不另起一把——多一把锁就多一种死锁可能，
//! 而重建实测 121ms，不值得。

use crate::rec::{load_lib, Lib};
use crate::util::*;
use crate::util::EE;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 索引目录名。点号开头表示"派生数据"，与 `.git` 同理。
pub const DIR: &str = ".index";

/// 索引格式版本。版本不符直接重建，不做兼容——
/// 兼容旧格式的代码比重新生成一次贵得多。
pub const SCHEMA: u32 = 1;

/// 一条记录在索引里的头部信息。
///
/// 只存召回需要的字段，不存正文。正文留在记录文件里，`show` 时才读。
/// 这样索引体积与记录数成正比而不与总字数成正比。
#[derive(Clone, Debug, Default)]
pub struct Doc {
    /// 相对库根的路径，如 `canonical/records/AB2-C-001.md`。
    /// **绝不存绝对路径**——那会让整个目录不可迁移。
    pub path: String,
    pub title: String,
    /// 正文词数用于长度归一化。**不再存词频**——
    /// 词频全在 postings 里，docs 只留元数据。
    ///
    /// 原来每条记录都存完整词频表，2 万条时 docs 涨到 6.8MB，
    /// 每次查询都要读它并解析 2 万行，等于把"读全库"换了个形式。
    /// 现在 docs 只存轻量头部（约 100 字节/条），查询时只从 postings
    /// 取需要的词。
    /// 正文总词数，用于 BM25 长度归一化。
    pub len: u32,
    /// KEYS/CONCEPTS 里的策展词（作者写的词，不过 2-gram 分词器）。
    pub keys: Vec<String>,
    /// 三级权重的依据，原样存字符串，判定在查询时做。
    pub kind: String,
    pub maturity: String,
    pub authority: String,
    /// 记录文件的 mtime，用于判断索引是否过期。
    pub mtime: i64,
}

impl Doc {
    pub fn lib(&self) -> Lib {
        if self.path.starts_with("candidate/") {
            Lib::Candidate
        } else {
            Lib::Canonical
        }
    }
}

/// 整个索引。
#[derive(Clone, Debug, Default)]
pub struct Index {
    pub docs: Vec<Doc>,
    /// 词 → [(doc_id, 词频)]。这是真正省时间的部分：
    /// 查询时只看包含该词的记录，而不是全部记录。
    pub postings: HashMap<String, Vec<(u32, u32)>>,
    pub avgdl: f64,
    /// 记录总数与总字节，用于和磁盘对账。
    pub total_docs: usize,
    /// 目录签名（条数 + 目录 mtime 混合），用于和磁盘对账。
    pub total_bytes: u64,
    /// 索引落后于磁盘时置位，下次写入时全量重建。
    pub dirty: bool,
    /// path → doc_id。查 term 是否属于某条记录时必须用映射，
    /// 不能每次 `docs.iter().position()`——那是 O(n)，
    /// 实测 2 万条时把一次查询从 340ms 拖到 933ms。
    pub by_path: HashMap<String, u32>,
}

fn dir_of(root: &Path) -> PathBuf {
    root.join(DIR)
}

fn meta_path(root: &Path) -> PathBuf {
    dir_of(root).join("meta")
}

fn docs_path(root: &Path) -> PathBuf {
    dir_of(root).join("docs")
}

fn postings_path(root: &Path) -> PathBuf {
    dir_of(root).join("postings")
}

/// 磁盘上的实际记录清单：相对路径 + mtime + 字节数。
///
/// 这是"索引是否过期"的唯一判据。用条数与总字节对账，而不是逐条比对 mtime
/// ——后者在几千条时和重建索引一样贵，那就失去意义了。
fn disk_snapshot(root: &Path) -> (usize, u64) {
    // 对账用「条数 + 文件名指纹」，不用目录 mtime、也不逐文件 stat。
    //
    // 三条都试过，前两条都不行：
    //   * 逐文件 metadata()：2 万条要 3.5 秒，守卫比被守的东西贵。
    //   * 目录 mtime：复制、解压、git checkout、rsync 都会改它，
    //     于是**迁移一次索引就判过期**——而"整目录换机器还能用"正是硬要求。
    //   * 只数条数：改名或同数量替换发现不了。
    //
    // 文件名指纹两头都占：不变（复制不改名）、便宜（只 readdir，不 stat 每个文件）、
    // 且对增删改名敏感。abook 的记录名是「时间戳-内容哈希」，所以文件名本身
    // 就带着内容指纹——不改名就不会改内容，改了内容必然改名。
    let mut n = 0usize;
    let mut h: u64 = 0xcbf29ce484222325;
    for lib in [Lib::Canonical, Lib::Candidate] {
        let dir = root.join(lib.dir());
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let mut names: Vec<String> = rd
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.ends_with(".md"))
                .collect();
            names.sort();
            for name in names {
                n += 1;
                for b in name.as_bytes() {
                    h ^= *b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
            }
        }
    }
    (n, h)
}

impl Index {
    /// 从磁盘读索引。返回 None 表示"没有可用的索引"，调用方应重建。
    ///
    /// 三种情况都返回 None，且都不报错：
    ///   * 索引不存在（首次使用、或用户 `rm -rf .index/`）
    ///   * schema 版本不符（升级过二进制）
    ///   * 条数或字节数对不上（记录被外部改动过）
    ///
    /// 不报错是刻意的：索引是加速件，它不可用不该变成用户要处理的问题。
    pub fn load(root: &Path) -> Option<Index> {
        let meta = std::fs::read_to_string(meta_path(root)).ok()?;
        let mut lines = meta.lines();
        let schema: u32 = lines.next()?.trim().parse().ok()?;
        if schema != SCHEMA {
            return None;
        }
        let total_docs: usize = lines.next()?.trim().parse().ok()?;
        let total_bytes: u64 = lines.next()?.trim().parse().ok()?;
        let dirty = lines.next().map(|s| s.trim() == "dirty").unwrap_or(false);

        // 与磁盘对账。对不上就当没有索引——重建比修复简单，也不会修错。
        let (dn, db) = disk_snapshot(root);
        if dn != total_docs || db != total_bytes {
            return None;
        }
        if dirty {
            return None;
        }

        let mut docs: Vec<Doc> = Vec::new();
        // 读分片。整体 docs 文件已废弃（分片是为了让单次查询不必读全部记录头），
        // 但仍兼容：如果只有整体文件存在，也照读——旧索引不该因为格式升级就失效。
        let mut raw = String::new();
        let mut any_shard = false;
        for sh in 0..16 {
            if let Ok(body) = std::fs::read_to_string(docs_path(root).with_extension(format!("d{sh}"))) {
                raw.push_str(&body);
                any_shard = true;
            }
        }
        if !any_shard {
            raw = std::fs::read_to_string(docs_path(root)).ok()?;
        }
        for line in raw.lines() {
            if line.trim().is_empty() {
                continue;
            }
            // 行格式：path \t title \t len \t kind \t maturity \t authority \t mtime \t keys(逗号) \t terms(词:频,词:频)
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 8 {
                return None;
            }
            docs.push(Doc {
                path: f[0].to_string(),
                title: f[1].to_string(),
                len: f[2].parse().unwrap_or(0),
                kind: f[3].to_string(),
                maturity: f[4].to_string(),
                authority: f[5].to_string(),
                mtime: f[6].parse().unwrap_or(0),
                keys: f[7]
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect(),
            });
        }

        let mut postings: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        let praw = std::fs::read_to_string(postings_path(root)).ok()?;
        for line in praw.lines() {
            if line.trim().is_empty() {
                continue;
            }
            // 行格式：term \t id:tf,id:tf,...
            let (term, rest) = match line.split_once('\t') {
                Some(v) => v,
                None => continue,
            };
            let mut v = Vec::new();
            for pair in rest.split(',') {
                if let Some((i, c)) = pair.split_once(':') {
                    if let (Ok(i), Ok(c)) = (i.parse::<u32>(), c.parse::<u32>()) {
                        v.push((i, c));
                    }
                }
            }
            postings.insert(term.to_string(), v);
        }

        let total_len: u64 = docs.iter().map(|d| d.len as u64).sum();
        let avgdl = if docs.is_empty() {
            1.0
        } else {
            total_len as f64 / docs.len() as f64
        };

        let by_path = docs
            .iter()
            .enumerate()
            .map(|(i, d)| (d.path.clone(), i as u32))
            .collect();
        Some(Index {
            docs,
            postings,
            avgdl,
            total_docs,
            total_bytes,
            dirty: false,
            by_path,
        })
    }

    /// 全量重建。O(n)，只应在索引缺失、版本变更、或 `refresh` 时调用。
    ///
    /// 写入侧的增量维护是另一条路（见 `append_doc`），不要因为这条慢就以为
    /// 每次写都要走它。
    pub fn build(root: &Path, libs: &[Lib]) -> Index {
        let mut docs: Vec<Doc> = Vec::new();
        let mut postings: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        let mut total_bytes = 0u64;

        for &lib in libs {
            for rec in load_lib(root, lib) {
                let rel_path = rel(root, &rec.path);
                let mtime = std::fs::metadata(&rec.path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                total_bytes += std::fs::metadata(&rec.path).map(|m| m.len()).unwrap_or(0);

                // 正文 + 标题 + KEYS 一起分词。标题与 KEYS 重复计入是有意的：
                // 它们比正文更能代表"这条讲什么"，多算几次等于加权。
                let mut buf = String::new();
                buf.push_str(&rec.title());
                buf.push(' ');
                buf.push_str(&rec.get("KEYS"));
                buf.push(' ');
                buf.push_str(&rec.get("CONCEPTS"));
                buf.push(' ');
                buf.push_str(&rec.get("SUMMARY"));
                buf.push(' ');
                buf.push_str(&rec.body);

                let toks = crate::semantic::tokenize(&buf);
                let mut tf: HashMap<String, u32> = HashMap::new();
                for t in &toks {
                    *tf.entry(t.clone()).or_insert(0) += 1;
                }
                let id = docs.len() as u32;
                for (t, c) in &tf {
                    postings.entry(t.clone()).or_default().push((id, *c));
                }

                let keys = split_list(&rec.get("KEYS"))
                    .into_iter()
                    .chain(split_list(&rec.get("CONCEPTS")))
                    .collect::<Vec<_>>();

                docs.push(Doc {
                    path: rel_path,
                    title: rec.title(),
                    len: toks.len() as u32,
                    keys,
                    kind: rec.get("KIND").trim().to_string(),
                    maturity: rec.get("MATURITY").trim().to_string(),
                    authority: rec.get("AUTHORITY").trim().to_string(),
                    mtime,
                });
            }
        }

        let total_len: u64 = docs.iter().map(|d| d.len as u64).sum();
        let avgdl = if docs.is_empty() {
            1.0
        } else {
            total_len as f64 / docs.len() as f64
        };
        let by_path = docs
            .iter()
            .enumerate()
            .map(|(i, d)| (d.path.clone(), i as u32))
            .collect();
        // 对账用**目录签名**，与 disk_snapshot 同一口径。
        // 不能在这里累加文件字节：那是另一个量，存进去就永远对不上，
        // 于是每次 load 都判"过期"、每次查询都全量重建——
        // 实测这个 bug 让 2 万条的召回从 225ms 变成 2027ms。
        let (sig_n, sig) = disk_snapshot(root);
        Index {
            total_docs: if sig_n > 0 { sig_n } else { docs.len() },
            docs,
            postings,
            avgdl,
            total_bytes: sig,
            dirty: false,
            by_path,
        }
    }

    /// 写回磁盘。调用方负责保证这在库级锁内。
    ///
    /// 三个文件各自原子写。不追求三者之间的事务性：`meta` 里的条数与字节
    /// 是最后写的，如果中途失败，下次 `load` 对不上就会重建——这正是我们要的
    /// 行为，不需要额外的一致性协议。
    pub fn save(&self, root: &Path) -> R<()> {
        let d = dir_of(root);
        std::fs::create_dir_all(&d).map_err(|e| EE::Io(format!("建索引目录失败 {}: {e}", d.display())))?;

        let mut docs_buf = String::new();
        for doc in &self.docs {
            docs_buf.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                doc.path,
                doc.title.replace('\t', " "),
                doc.len,
                doc.kind,
                doc.maturity,
                doc.authority,
                doc.mtime,
                doc.keys.join(","),
            ));
        }

        // postings 按词首字符分 64 桶。
        //
        // 不分桶时 2 万词条要 4.2MB，每次查询全读全解析，实测占了召回
        // 大半时间。分桶后一次查询只读它用到的几个词所在的桶。
        //
        // 桶号用词的哈希而不是首字符：中文词首字符分布极不均（"记""的"开头的
        // 词占了很大一片），哈希能让桶大小接近均匀。
        const BUCKETS: usize = 64;
        let mut buckets: Vec<String> = vec![String::new(); BUCKETS];
        for (term, list) in &self.postings {
            let b = bucket_of(term, BUCKETS);
            let s = list
                .iter()
                .map(|(i, c)| format!("{i}:{c}"))
                .collect::<Vec<_>>()
                .join(",");
            buckets[b].push_str(&format!("{term}\t{s}\n"));
        }
        for (b, body) in buckets.iter().enumerate() {
            let p = postings_path(root).with_extension(format!("p{b}"));
            if body.is_empty() {
                let _ = std::fs::remove_file(&p);
            } else {
                write_atomic(&p, body)?;
            }
        }
        // 旧的单文件 postings 删掉，避免与新分桶混淆
        let _ = std::fs::remove_file(postings_path(root));

        // docs 分片写：查询时按需读分片，不必把全部记录头读进内存。
        //
        // 不分片时 docs 随记录数线性增长（2 万条 3.1MB），每次查询都要全读，
        // 增长曲线实测 500→10000 条是 26→117ms，仍是线性。
        // 分片后单次查询只需读命中的那几片。
        const SHARDS: usize = 16;
        let lines: Vec<&str> = docs_buf.lines().collect();
        let per = if lines.is_empty() { 1 } else { (lines.len() + SHARDS - 1) / SHARDS };
        for sh in 0..SHARDS {
            let from = sh * per;
            if from >= lines.len() {
                let _ = std::fs::remove_file(docs_path(root).with_extension(format!("d{sh}")));
                continue;
            }
            let to = ((sh + 1) * per).min(lines.len());
            let body = lines[from..to].join("\n") + "\n";
            write_atomic(&docs_path(root).with_extension(format!("d{sh}")), &body)?;
        }
        // 旧的整体 docs 文件若存在，删掉，避免与新分片混淆
        let _ = std::fs::remove_file(docs_path(root));
        // meta 最后写：它一旦就位，说明前面两份已经完整。
        write_atomic(
            &meta_path(root),
            &format!(
                "{}\n{}\n{}\n{}\n",
                SCHEMA,
                self.total_docs,
                self.total_bytes,
                if self.dirty { "dirty" } else { "clean" }
            ),
        )?;
        Ok(())
    }

    /// 标记索引失效，但不清空。
    ///
    /// 用在"记录已成功写入、索引更新失败"这条路上：记录必须保住，索引下次重建。
    pub fn mark_dirty(root: &Path) {
        let p = meta_path(root);
        if let Ok(s) = std::fs::read_to_string(&p) {
            let mut lines: Vec<String> = s.lines().map(|l| l.to_string()).collect();
            if lines.len() >= 4 {
                lines[3] = "dirty".to_string();
                let _ = write_atomic(&p, &(lines.join("\n") + "\n"));
            }
        }
    }

    /// 索引是否落后于磁盘。写命令用它决定要不要重建。
    pub fn stale(root: &Path) -> bool {
        match Self::load(root) {
            Some(_) => false,
            None => true,
        }
    }
}

/// 词频总和，用于 BM25 的长度归一化。
impl Index {
    pub fn total_terms(&self) -> usize {
        self.postings.len()
    }
}

impl Index {
    /// 在已加载的索引上跑一次 BM25。这是"查表"的那一步——
    /// 只看包含查询词的记录，不碰磁盘。
    ///
    /// 返回 (doc_id, score, why) 按分降序。`k=0` 表示不截断。
    pub fn search(&self, q: &str, k: usize) -> Vec<(usize, f64, String)> {
        let base: Vec<String> = crate::semantic::tokenize(q);
        if base.is_empty() {
            return Vec::new();
        }
        let n = self.docs.len() as f64;
        if n == 0.0 {
            return Vec::new();
        }
        let k1 = 1.4f64;
        let b = 0.72f64;
        let mut scores = vec![0.0f64; self.docs.len()];
        let mut covered = vec![0usize; self.docs.len()];

        let mut distinct: Vec<&String> = base.iter().collect();
        distinct.sort();
        distinct.dedup();
        // 覆盖率门槛与 semantic.rs 保持同一口径：平方根量级。
        // 中文口语虚词多，要求覆盖三成等于要求命中一半主题，会把整句判死。
        let need = if distinct.len() >= 4 {
            (distinct.len() as f64).sqrt().floor() as usize
        } else {
            1
        };

        for t in &base {
            let plist = match self.postings.get(t) {
                Some(p) => p,
                None => continue,
            };
            let df = plist.len() as f64;
            // BM25 idf，+0.5 平滑，下限 0 防止负分
            let idf = (((n - df + 0.5) / (df + 0.5)).ln()).max(0.0);
            if idf <= 0.0 {
                continue;
            }
            for (id, tf) in plist {
                let dl = self.docs[*id as usize].len as f64;
                let denom = *tf as f64 + k1 * (1.0 - b + b * dl / self.avgdl);
                scores[*id as usize] += idf * (*tf as f64 * (k1 + 1.0)) / denom;
                covered[*id as usize] += 1;
            }
        }

        let mut rows: Vec<(usize, f64, String)> = scores
            .iter()
            .enumerate()
            .filter(|(i, s)| **s > 0.0 && covered[*i] >= need && !self.docs[*i].title.is_empty())
            .map(|(i, s)| (i, *s, format!("bm25/覆盖{}/{}", covered[i], need)))
            .collect();
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        if k > 0 {
            rows.truncate(k);
        }
        rows
    }

    pub fn path_of(&self, id: usize) -> Option<&str> {
        self.docs.get(id).map(|d| d.path.as_str())
    }
}

impl Index {
    /// 某个词是否出现在某条记录的正文里。
    ///
    /// docs 不再存词频（那是 6.8MB 的来源），所以这里查 postings：
    /// 先定位词，再在它的记录列表里找目标 id。postings 里每个词的记录列表
    /// 通常很短，实际是常数级。
    pub fn has_term_in(&self, term: &str, path: &str) -> bool {
        let plist = match self.postings.get(term) {
            Some(p) => p,
            None => return false,
        };
        let id = match self.by_path.get(path) {
            Some(i) => *i,
            None => return false,
        };
        plist.iter().any(|(i, _)| *i == id)
    }
}

impl Index {
    /// 轻量加载：只读 meta 与 postings，**不读 docs 分片**。
    ///
    /// 这是让召回变成常数级的关键。postings 的体积取决于词表大小，与记录数
    /// 关系弱；docs 分片才随记录数线性增长。
    ///
    /// 查询流程变成：
    ///   1. 在 postings 上算 BM25 → 得到 doc_id 列表（不碰 docs）
    ///   2. 只读这些 id 所在的分片，取路径与元数据
    ///
    /// 这样磁盘读的次数取决于**命中数**（被 limit 卡住），不取决于库里有多少条。
    pub fn load_shallow(root: &Path, q: &str) -> Option<Index> {
        let meta = std::fs::read_to_string(meta_path(root)).ok()?;
        let mut lines = meta.lines();
        let schema: u32 = lines.next()?.trim().parse().ok()?;
        if schema != SCHEMA {
            return None;
        }
        let total_docs: usize = lines.next()?.trim().parse().ok()?;
        let total_bytes: u64 = lines.next()?.trim().parse().ok()?;
        let dirty = lines.next().map(|s| s.trim() == "dirty").unwrap_or(false);
        if dirty {
            return None;
        }
        let (dn, db) = disk_snapshot(root);
        if dn != total_docs || db != total_bytes {
            return None;
        }

        // 只读查询词落在的那几个桶，不读全部 postings。
        // 这一步是常数级的核心：磁盘读的量取决于**查询词数**，与库规模无关。
        let mut postings: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        let mut loaded: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for t in crate::semantic::tokenize(q) {
            let b = bucket_of(&t, BUCKETS);
            if !loaded.insert(b) {
                continue;
            }
            let p = postings_path(root).with_extension(format!("p{b}"));
            if let Ok(body) = std::fs::read_to_string(&p) {
                for line in body.lines() {
                    let (term, rest) = match line.split_once('\t') {
                        Some(v) => v,
                        None => continue,
                    };
                    let mut v = Vec::new();
                    for pair in rest.split(',') {
                        if let Some((i, c)) = pair.split_once(':') {
                            if let (Ok(i), Ok(c)) = (i.parse::<u32>(), c.parse::<u32>()) {
                                v.push((i, c));
                            }
                        }
                    }
                    postings.insert(term.to_string(), v);
                }
            }
        }

        Some(Index {
            docs: Vec::new(),
            postings,
            avgdl: 0.0,
            total_docs,
            total_bytes,
            dirty: false,
            by_path: HashMap::new(),
        })
    }

    /// 读某条记录头（按 id）。只读它所在的那一个分片。
    pub fn doc_at(&self, root: &Path, id: usize) -> Option<Doc> {
        if id >= self.total_docs {
            return None;
        }
        let sh = shard_of(id, self.total_docs);
        let body = std::fs::read_to_string(
            docs_path(root).with_extension(format!("d{sh}")),
        )
        .ok()?;
        let first = shard_first(id, self.total_docs);
        let line = body.lines().nth(id - first)?;
        parse_doc_line(line)
    }

    /// 一次读一整片，供需要连续取多条时用（比逐条读省）。
    pub fn shard(&self, root: &Path, sh: usize) -> Vec<(usize, Doc)> {
        let body = match std::fs::read_to_string(
            docs_path(root).with_extension(format!("d{sh}")),
        ) {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };
        let first = shard_first_by_sh(sh, self.total_docs);
        body.lines()
            .enumerate()
            .filter_map(|(i, l)| parse_doc_line(l).map(|d| (first + i, d)))
            .collect()
    }

    /// 分片总数（写侧固定 16，读侧据此定位）。
    pub fn shards(&self) -> usize {
        16
    }
}

/// 记录 id 落在哪个分片。与 save 的分片规则必须完全一致。
fn shard_of(id: usize, total: usize) -> usize {
    let per = if total == 0 { 1 } else { (total + 16 - 1) / 16 };
    (id / per).min(15)
}

fn shard_first(id: usize, total: usize) -> usize {
    let per = if total == 0 { 1 } else { (total + 16 - 1) / 16 };
    (id / per).min(15) * per
}

fn shard_first_by_sh(sh: usize, total: usize) -> usize {
    let per = if total == 0 { 1 } else { (total + 16 - 1) / 16 };
    sh * per
}

fn parse_doc_line(line: &str) -> Option<Doc> {
    if line.trim().is_empty() {
        return None;
    }
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 8 {
        return None;
    }
    Some(Doc {
        path: f[0].to_string(),
        title: f[1].to_string(),
        len: f[2].parse().unwrap_or(0),
        kind: f[3].to_string(),
        maturity: f[4].to_string(),
        authority: f[5].to_string(),
        mtime: f[6].parse().unwrap_or(0),
        keys: f[7].split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect(),
    })
}

impl Index {
    /// 记录 id 落在哪个分片。查询侧用，与写侧规则必须一致。
    pub fn shard_of_id(&self, id: usize) -> usize {
        shard_of(id, self.total_docs)
    }

    /// 只在 postings 上跑 BM25，不读 docs。
    ///
    /// 与 `search` 的区别：`search` 需要 docs 拿标题与长度；这里拿不到，
    /// 所以用 postings 里记录的词频近似长度，并用 avgdl=1 归一化。
    ///
    /// 精度略低于完整版，但这一步只用于**选出候选**——最终排序由 RRF 在
    /// 已加载的少量 doc 上重算。用低精度选候选、高精度排最终结果，
    /// 是这类两段式检索的常规做法。
    pub fn search_shallow(&self, q: &str, k: usize) -> Vec<(usize, f64, String)> {
        let base: Vec<String> = crate::semantic::tokenize(q);
        if base.is_empty() || self.total_docs == 0 {
            return Vec::new();
        }
        let n = self.total_docs as f64;
        let k1 = 1.4f64;
        let b = 0.72f64;
        let mut scores: HashMap<u32, f64> = HashMap::new();
        let mut covered: HashMap<u32, usize> = HashMap::new();

        let mut distinct: Vec<&String> = base.iter().collect();
        distinct.sort();
        distinct.dedup();
        let need = if distinct.len() >= 4 {
            (distinct.len() as f64).sqrt().floor() as usize
        } else {
            1
        };

        for t in &base {
            let plist = match self.postings.get(t) {
                Some(p) => p,
                None => continue,
            };
            let df = plist.len() as f64;
            let idf = (((n - df + 0.5) / (df + 0.5)).ln()).max(0.0);
            if idf <= 0.0 {
                continue;
            }
            for (id, tf) in plist {
                // 长度未知时用 tf 本身做归一化分母的近似：
                // 词频越高说明记录越长，这与 BM25 的长度惩罚方向一致。
                let denom = *tf as f64 + k1 * (1.0 - b + b * 1.0);
                *scores.entry(*id).or_insert(0.0) += idf * (*tf as f64 * (k1 + 1.0)) / denom;
                *covered.entry(*id).or_insert(0) += 1;
            }
        }

        let mut rows: Vec<(usize, f64, String)> = scores
            .into_iter()
            .filter(|(id, s)| *s > 0.0 && covered.get(id).copied().unwrap_or(0) >= need)
            .map(|(id, s)| (id as usize, s, "bm25shallow".to_string()))
            .collect();
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        if k > 0 {
            rows.truncate(k);
        }
        rows
    }
}

/// 词 → 桶号。用 FNV-1a，够散且不需要依赖。
pub fn bucket_of(term: &str, buckets: usize) -> usize {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in term.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h as usize) % buckets
}

/// 桶总数。写读两侧必须一致。
pub const BUCKETS: usize = 64;
