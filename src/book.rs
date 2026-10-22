//! Global.md and Index.md share one wrapper format:
//!   [MEMORY_BOOK_V2] ... [/MEMORY_BOOK_V2]
//!   [MEMORY_CONTENT] ... [/MEMORY_CONTENT]
//! Trailing content after the final tag is preserved verbatim.

use crate::util::*;
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct BookFile {
    pub path: String,
    pub meta: Vec<(String, String)>,
    pub meta_rows: Vec<String>,   // "1|Global.md|入口|# Global.md"
    pub content: String,
    pub trailing: String,
}

impl BookFile {
    pub fn load(p: &Path) -> R<BookFile> {
        let raw = read_str(p)?;
        let mut bf = BookFile {
            path: p.to_string_lossy().to_string(),
            ..Default::default()
        };
        let mut state = 0u8; // 0 out, 1 meta, 2 content
        for line in raw.lines() {
            let t = line.trim();
            if t == "[MEMORY_BOOK_V2]" {
                state = 1;
                continue;
            }
            if t == "[/MEMORY_BOOK_V2]" {
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
        Ok(bf)
    }

    pub fn render(&self) -> String {
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
        self.content = out.join("\n");
        if !self.content.ends_with('\n') {
            self.content.push('\n');
        }
        true
    }

    /// Index rows from the "## Active Routes" table: (path, summary, when).
    pub fn routes(&self) -> Vec<(String, String, String)> {
        let mut v = Vec::new();
        let mut insec = false;
        for line in self.content.lines() {
            let t = line.trim();
            if let Some(h) = t.strip_prefix("## ") {
                insec = h.trim().eq_ignore_ascii_case("Active Routes");
                continue;
            }
            if !insec || t.is_empty() {
                continue;
            }
            if t.starts_with('|') || t.contains("路径｜用途") {
                continue;
            }
            let parts: Vec<&str> = t.split('｜').collect();
            if parts.len() >= 2 {
                let p = parts[0].trim().to_string();
                let s = parts[1].trim().to_string();
                let w = parts.get(2).map(|x| x.trim().to_string()).unwrap_or_default();
                v.push((p, s, w));
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
        self.content = out.join("\n");
        if !self.content.ends_with('\n') {
            self.content.push('\n');
        }
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
        self.content = out.join("\n");
        if !self.content.ends_with('\n') {
            self.content.push('\n');
        }
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
