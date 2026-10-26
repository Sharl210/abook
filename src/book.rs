//! Global.md and Index.md share one wrapper format:
//!   [MEMORY_BOOK_V2] ... [/MEMORY_BOOK_V2]
//!   [MEMORY_CONTENT] ... [/MEMORY_CONTENT]
//! Trailing content after the final tag is preserved verbatim.
//!
//! The header pair is spelled two ways in practice: the format documentation
//! says `[MEMORY_BOOK_V2]`, while every real `Global.md` and `Index.md` in the
//! shipped library opens with `[MEMORY_FILE_CONTENTS]`. Both are accepted here;
//! parsing only the documented one left the real headers invisible.

use crate::util::*;
use std::path::Path;

/// Opening tags of the header block, as found in real files and in the spec.
const HEADER_OPEN: &[&str] = &["[MEMORY_FILE_CONTENTS]", "[MEMORY_BOOK_V2]"];
/// Closing tags of the header block.
const HEADER_CLOSE: &[&str] = &["[/MEMORY_FILE_CONTENTS]", "[/MEMORY_BOOK_V2]"];

#[derive(Clone, Debug, Default)]
pub struct BookFile {
    pub meta: Vec<(String, String)>,
    pub meta_rows: Vec<String>,   // "1|Global.md|入口|# Global.md"
    pub content: String,
    pub trailing: String,
    /// Everything up to and including the `[MEMORY_CONTENT]` line, byte-exact.
    ///
    /// Global.md and Index.md are user files, not our own format, so a write
    /// must not reformat them. Rebuilding the header from parsed fields used
    /// to drop the blank lines inside the meta block -- a real library file
    /// lost three unrelated lines that way. Keeping the original bytes and
    /// replacing only the content section means an edit touches exactly one
    /// line and nothing else.
    pub head_raw: String,
    /// From the `[/MEMORY_CONTENT]` line to EOF, byte-exact.
    pub tail_raw: String,
}

impl BookFile {
    pub fn load(p: &Path) -> R<BookFile> {
        let raw = read_str(p)?;
        let mut bf = BookFile {
            ..Default::default()
        };
        let mut state = 0u8; // 0 out, 1 meta, 2 content -> 3 tail
        for line in raw.lines() {
            let t = line.trim();
            // Two header tag pairs exist in the wild: the format doc names
            // `[MEMORY_BOOK_V2]`, but every real Global.md and Index.md opens
            // with `[MEMORY_FILE_CONTENTS]`. Recognizing only the documented pair
            // meant the header of the shipped library never parsed -- and since
            // `render()` splices `head_raw` back verbatim, a write silently
            // discarded every header field it thought it had set.
            if HEADER_OPEN.contains(&t) {
                state = 1;
                continue;
            }
            if HEADER_CLOSE.contains(&t) {
                state = 0;
                continue;
            }
            if t == "[MEMORY_CONTENT]" {
                state = 2;
                continue;
            }
            if t == "[/MEMORY_CONTENT]" {
                state = 3;
                continue;
            }
            match state {
                1 => {
                    if t.is_empty() {
                        continue;
                    }
                    if t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && t.contains('|')
                    {
                        bf.meta_rows.push(t.to_string());
                    } else if let Some((k, v)) = t.split_once('=') {
                        bf.meta.push((k.trim().to_string(), v.trim().to_string()));
                    }
                }
                2 => {
                    bf.content.push_str(line);
                    bf.content.push('\n');
                }
                3 => {
                    if !line.trim().is_empty() || !bf.trailing.is_empty() {
                        bf.trailing.push_str(line);
                        bf.trailing.push('\n');
                    }
                }
                _ => {}
            }
        }
        bf.head_raw = match raw.find("[MEMORY_CONTENT]") {
            Some(i) => {
                let after = raw[i..].find('\n').map(|k| i + k + 1).unwrap_or(raw.len());
                raw[..after].to_string()
            }
            None => String::new(),
        };
        bf.tail_raw = match raw.rfind("[/MEMORY_CONTENT]") {
            Some(i) => raw[i..].to_string(),
            None => String::new(),
        };
        Ok(bf)
    }

    pub fn render(&self) -> String {
        // Loaded from disk: splice the (possibly edited) content section back
        // into the original bytes, so the header and tail stay byte-identical.
        if !self.head_raw.is_empty() && !self.tail_raw.is_empty() {
            let mut s = String::with_capacity(self.head_raw.len() + self.content.len() + self.tail_raw.len());
            s.push_str(&self.head_raw);
            s.push_str(&self.content);
            if !self.content.is_empty() && !self.content.ends_with('\n') {
                s.push('\n');
            }
            s.push_str(&self.tail_raw);
            return s;
        }
        let mut s = String::new();
        s.push_str("[MEMORY_BOOK_V2]\n");
        for (k, v) in &self.meta {
            s.push_str(&format!("{k}={v}\n"));
        }
        for r in &self.meta_rows {
            s.push_str(r);
            s.push('\n');
        }
        s.push_str("[/MEMORY_BOOK_V2]\n\n");
        s.push_str("[MEMORY_CONTENT]\n");
        s.push_str(&self.content);
        if !self.content.ends_with('\n') {
            s.push('\n');
        }
        s.push_str("[/MEMORY_CONTENT]\n");
        if !self.trailing.trim().is_empty() {
            s.push_str(&self.trailing);
        }
        s
    }

    /// Replace the content section, preserving how many newlines it ended with.
    ///
    /// `str::lines()` drops trailing blank lines, so the usual
    /// `lines().collect().join("\n")` round-trip quietly turns a file ending
    /// in "\n\n" into one ending in "\n" -- one unrelated blank line lost per
    /// write. Counting the original trailing newlines and restoring exactly
    /// that many keeps every write local to the line it means to touch.
    fn set_content_lines(&mut self, out: &[String]) {
        let tail_nl = self.content.chars().rev().take_while(|c| *c == '\n').count();
        let mut s = out.join("\n");
        while s.ends_with('\n') {
            s.pop();
        }
        for _ in 0..tail_nl {
            s.push('\n');
        }
        self.content = s;
    }

    /// Value of `KEY=` in the header block (Global.md / Index.md entry header).
    ///
    /// Separate from [`state`](Self::state): the real files carry the record
    /// counts **twice**, once in the header (`CANDIDATE_RECORDS=437`) and once in
    /// the body's `## Current State` (`CANDIDATE_RECORDS=438`). The two drifting
    /// apart is what made the tool report a count the file itself contradicted.
    pub fn header(&self, k: &str) -> Option<String> {
        for line in self.head_raw.lines() {
            if let Some((kk, v)) = line.split_once('=') {
                if kk.trim() == k {
                    return Some(v.trim().to_string());
                }
            }
        }
        None
    }

    /// Set `KEY=` in the header block, editing only that one line.
    ///
    /// Works on `head_raw`, which `render()` splices back byte-for-byte, so this
    /// is the only way a header field can actually change. Missing keys are
    /// inserted just before the header's closing tag, keeping the block intact.
    pub fn set_header(&mut self, k: &str, v: &str) -> bool {
        if self.head_raw.is_empty() {
            // No header block at all: fall back to the meta list, which `render`
            // writes out when it has no raw bytes to preserve.
            if let Some(e) = self.meta.iter_mut().find(|(kk, _)| kk == k) {
                e.1 = v.to_string();
            } else {
                self.meta.push((k.to_string(), v.to_string()));
            }
            return true;
        }
        let mut lines: Vec<String> = self.head_raw.lines().map(|s| s.to_string()).collect();
        let mut hit = false;
        for line in lines.iter_mut() {
            if let Some((kk, _)) = line.split_once('=') {
                // Only rewrite a field line, not prose that happens to contain `=`.
                if kk.trim() == k && !kk.trim().starts_with(['#', '-', '*', '|']) {
                    *line = format!("{k}={v}");
                    hit = true;
                    break;
                }
            }
        }
        if !hit {
            let pos = lines
                .iter()
                .position(|l| HEADER_CLOSE.contains(&l.trim()))
                .unwrap_or(lines.len());
            lines.insert(pos, format!("{k}={v}"));
        }
        let mut out = lines.join("\n");
        if self.head_raw.ends_with('\n') {
            out.push('\n');
        }
        self.head_raw = out;
        true
    }

    /// Update a count-style key everywhere the file carries it: header and body.
    ///
    /// Returns whether anything was written. Writing only the body occurrence (the
    /// old behaviour) left `Global.md` stating two different record counts and
    /// never touched `Index.md` at all, which has the field only in its header.
    pub fn set_everywhere(&mut self, k: &str, v: &str) -> bool {
        let mut wrote = false;
        if self.header(k).is_some() {
            self.set_header(k, v);
            wrote = true;
        }
        if self.state(k).is_some() {
            self.set_state(k, v);
            wrote = true;
        }
        if !wrote {
            // Neither place has it yet: put it in the header, which is the
            // authoritative spot for facts about the library as a whole.
            self.set_header(k, v);
            wrote = true;
        }
        wrote
    }

    /// Value of `KEY=` in the content section (Global.md Current State).
    pub fn state(&self, k: &str) -> Option<String> {
        for line in self.content.lines() {
            if let Some((kk, v)) = line.split_once('=') {
                if kk.trim() == k {
                    return Some(v.trim().to_string());
                }
            }
        }
        None
    }

    pub fn set_state(&mut self, k: &str, v: &str) -> bool {
        let mut out: Vec<String> = Vec::new();
        let mut done = false;
        for line in self.content.lines() {
            if let Some((kk, _)) = line.split_once('=') {
                if kk.trim() == k {
                    out.push(format!("{k}={v}"));
                    done = true;
                    continue;
                }
            }
            out.push(line.to_string());
        }
        if !done {
            // insert after the "## Current State" heading block start
            let mut inserted = false;
            let mut rebuilt: Vec<String> = Vec::new();
            for line in &out {
                rebuilt.push(line.clone());
                if !inserted && line.trim() == "## Current State" {
                    rebuilt.push(format!("{k}={v}"));
                    inserted = true;
                }
            }
            if !inserted {
                rebuilt.insert(0, format!("{k}={v}"));
            }
            out = rebuilt;
        }
        self.set_content_lines(&out);
        true
    }

    /// Index rows: (path, summary, when).
    ///
    /// Index.md 是给 AI 开工时看的那张「记忆库平面图」——有哪些分支、各自管
    /// 什么、什么时候该翻。没有它，AI 不知道该往哪儿搜，只能全库乱翻。
    ///
    /// 原来这个函数只认一种格式：`## Active Routes` 节下的全角竖线行
    /// （`路径｜用途｜何时读取`）。而真实库里的 Index.md 是 **markdown 表格**，
    /// 而且那些记录级路由行落在文末、前面没有 `## Active Routes` 标题。
    /// 两个条件都不满足，于是 `abook index` 对一份有 20 个分支路由的索引
    /// 报出「路由 0 条」——入口存在，内容读不出来。
    ///
    /// 现在两种格式都认，且不依赖特定小节标题：
    ///
    /// 1. **markdown 表格**：按表头判断列义，而不是按位置硬编码。表头里带
    ///    `BRANCH`/`分支` 的才算路由表——Index.md 里还有一张 `UNIT | 来源范围`
    ///    的迁移对照表，那张不是路由，不能混进来。
    /// 2. **全角竖线行**：`路径｜用途｜何时读取`，任意位置都算。
    pub fn routes(&self) -> Vec<(String, String, String)> {
        let mut v: Vec<(String, String, String)> = Vec::new();
        // 当前 markdown 表格的列映射：列序号 -> 语义
        let mut tbl: Option<(usize, usize, usize)> = None;
        for line in self.content.lines() {
            let t = line.trim();
            if t.is_empty() {
                tbl = None;
                continue;
            }
            // ---- markdown 表格 ----
            if t.starts_with('|') {
                let cells: Vec<String> = t
                    .trim_matches('|')
                    .split('|')
                    .map(|c| c.trim().to_string())
                    .collect();
                // 分隔行 |---|---|
                let is_sep = !cells.is_empty()
                    && cells.iter().all(|c| {
                        !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':')
                    });
                if is_sep {
                    continue;
                }
                // 表头行：判断这张表是不是路由表
                let looks_header = cells.iter().any(|c| {
                    let u = c.to_uppercase();
                    u.contains("BRANCH") || c.contains("分支")
                });
                if looks_header {
                    let find = |needles: &[&str]| -> usize {
                        cells
                            .iter()
                            .position(|c| {
                                let u = c.to_uppercase();
                                needles.iter().any(|n| u.contains(n))
                            })
                            .unwrap_or(usize::MAX)
                    };
                    let b = find(&["BRANCH", "分支"]);
                    let w = find(&["READ_WHEN", "READWHEN", "何时", "何时读取"]);
                    let n = find(&["正式", "候选", "COUNT", "记录数"]);
                    tbl = if b == usize::MAX {
                        None
                    } else {
                        Some((b, n, w))
                    };
                    continue;
                }
                if let Some((b, n, w)) = tbl {
                    let get = |i: usize| -> String {
                        if i == usize::MAX {
                            String::new()
                        } else {
                            cells
                                .get(i)
                                .map(|x| x.trim_matches('`').trim().to_string())
                                .unwrap_or_default()
                        }
                    };
                    let path = get(b);
                    if path.is_empty() {
                        continue;
                    }
                    let summary = get(n);
                    let when = get(w);
                    v.push((path, summary, when));
                }
                continue;
            }
            // 非表格行：表格结束
            tbl = None;
            // ---- 全角竖线行 ----
            if t.contains('｜') {
                let parts: Vec<&str> = t.split('｜').collect();
                // 表头字样本身不算路由
                if t.contains("路径｜用途") || t.contains("BRANCH｜") {
                    continue;
                }
                if parts.len() >= 2 {
                    let p = parts[0].trim().trim_matches('`').trim().to_string();
                    if p.is_empty() {
                        continue;
                    }
                    let s = parts[1].trim().to_string();
                    let w = parts.get(2).map(|x| x.trim().to_string()).unwrap_or_default();
                    v.push((p, s, w));
                }
            }
        }
        v
    }

    /// Insert or update one route row; keeps existing order.
    pub fn upsert_route(&mut self, path: &str, summary: &str, when: &str) -> bool {
        let row = format!("{path}｜{summary}｜{when}");
        let mut out: Vec<String> = Vec::new();
        let mut replaced = false;
        for line in self.content.lines() {
            let t = line.trim();
            let parts: Vec<&str> = t.split('｜').collect();
            if parts.len() >= 2 && parts[0].trim() == path {
                if summary.is_empty() {
                    out.push(line.to_string());
                } else {
                    out.push(row.clone());
                    replaced = true;
                }
                continue;
            }
            out.push(line.to_string());
        }
        if !replaced {
            // append at end of Active Routes section, else at file end
            let mut inserted = false;
            let mut rebuilt: Vec<String> = Vec::new();
            let mut insec = false;
            let mut last_row_idx: Option<usize> = None;
            for (i, line) in out.iter().enumerate() {
                let t = line.trim();
                if let Some(h) = t.strip_prefix("## ") {
                    insec = h.trim().eq_ignore_ascii_case("Active Routes");
                }
                if insec && t.split('｜').count() >= 2 {
                    last_row_idx = Some(i);
                }
                let _ = i;
            }
            if let Some(idx) = last_row_idx {
                for (i, line) in out.iter().enumerate() {
                    if i == idx + 1 {
                        rebuilt.push(row.clone());
                        inserted = true;
                    }
                    rebuilt.push(line.clone());
                }
                out = rebuilt;
            }
            if !inserted {
                out.push(row);
            }
        }
        self.set_content_lines(&out);
        true
    }

    /// Remove a route row by path.
    pub fn remove_route(&mut self, path: &str) -> bool {
        let mut out: Vec<String> = Vec::new();
        let mut removed = false;
        for line in self.content.lines() {
            let parts: Vec<&str> = line.trim().split('｜').collect();
            if parts.len() >= 2 && parts[0].trim() == path {
                removed = true;
                continue;
            }
            out.push(line.to_string());
        }
        self.set_content_lines(&out);
        removed
    }

    /// Credential identity index rows (Global.md only).
    pub fn credential_rows(&self) -> Vec<Vec<String>> {
        let mut v = Vec::new();
        let mut insec = false;
        for line in self.content.lines() {
            let t = line.trim();
            if let Some(h) = t.strip_prefix("## ") {
                insec = h.trim().eq_ignore_ascii_case("Credential Identity Index");
                continue;
            }
            if !insec || t.is_empty() {
                continue;
            }
            if t.starts_with("标识") {
                continue;
            }
            let parts: Vec<String> = t.split('｜').map(|x| x.trim().to_string()).collect();
            if parts.len() >= 3 {
                v.push(parts);
            }
        }
        v
    }
}
