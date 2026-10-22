//! Uniform pagination for every listing command.

use crate::util::Args;

#[derive(Clone, Copy, Debug)]
pub struct Page {
    pub page: usize,
    pub per: usize,
    pub total: usize,
    pub pages: usize,
}

impl Page {
    pub fn from_args(a: &Args, total: usize) -> Page {
        let per = a
            .num("per-page")
            .or_else(|| a.num("per"))
            .unwrap_or(20)
            .clamp(1, 5000) as usize;
        let pages = if total == 0 {
            1
        } else {
            (total + per - 1) / per
        };
        let mut page = a.num("page").unwrap_or(1).max(1) as usize;
        if page > pages {
            page = pages;
        }
        Page { page, per, total, pages }
    }

    /// Page covering a single item when `--page`/`--per-page` was not given.
    pub fn unchecked() -> Page {
        Page { page: 1, per: usize::MAX, total: 0, pages: 1 }
    }

    pub fn start(&self) -> usize {
        (self.page - 1).saturating_mul(self.per)
    }

    pub fn end(&self) -> usize {
        self.start().saturating_add(self.per).min(self.total)
    }

    pub fn sliced(&self) -> bool {
        self.total > self.per
    }

    /// One-line footer shown only when the result was actually cut.
    pub fn bar(&self) -> String {
        if !self.sliced() {
            return format!("共 {} 条", self.total);
        }
        let mut s = format!(
            "第 {}/{} 页  本页 {}-{}  共 {} 条",
            self.page,
            self.pages,
            self.start() + 1,
            self.end(),
            self.total
        );
        if self.page < self.pages {
            s.push_str(&format!("  （下一页 --page {}）", self.page + 1));
        }
        s
    }

    pub fn json_fields(&self) -> String {
        format!(
            "\"page\":{},\"per_page\":{},\"total\":{},\"pages\":{}",
            self.page, self.per, self.total, self.pages
        )
    }
}

/// Split a long text into `per`-line pages and return the requested page.
pub fn text_page(text: &str, a: &Args) -> (String, String) {
    let lines: Vec<&str> = text.lines().collect();
    let p = Page::from_args(a, lines.len());
    if !p.sliced() {
        return (text.to_string(), String::new());
    }
    let body = lines[p.start()..p.end()].join("\n");
    let footer = format!(
        "第 {}/{} 页  行 {}-{}  共 {} 行{}",
        p.page,
        p.pages,
        p.start() + 1,
        p.end(),
        p.total,
        if p.page < p.pages {
            format!("  （下一页 --page {}）", p.page + 1)
        } else {
            String::new()
        }
    );
    (body, footer)
}
