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
        // `--per-page 0` means "no pagination". It used to be clamped to 1, so
        // a caller asking for everything got a single row and no error -- the
        // worst kind of wrong answer, because it looks like the library only
        // holds one record. The default also disagreed with the documented 30.
        let per = match a.num("per-page").or_else(|| a.num("per")) {
            Some(0) => usize::MAX,
            Some(n) => n.clamp(1, 5000) as usize,
            None => 30,
        };
        // Guard the ceiling forms: with `per == usize::MAX` (the "no
        // pagination" sentinel) `total + per - 1` overflows and pages collapses
        // to 0, which then clamps the page to 0 and skips every row.
        let pages = if total == 0 || per == usize::MAX || per >= total {
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

    /// Paging fields only. Use this when the document already reports `total`
    /// under a more specific name (`hits`, `records`, `commits`): emitting it
    /// twice makes a strict JSON reader reject the document outright, and the
    /// lenient one silently keeps just one of the two.
    pub fn json_page_only(&self) -> String {
        format!(
            "\"page\":{},\"per_page\":{},\"pages\":{}",
            self.page, self.per, self.pages
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

/// Slice a list of already-rendered rows and tell the caller whether a footer
/// is needed.
///
/// Every "long list" command used to print its whole output no matter what
/// `--page` said, because `spec_for` added `page`/`per-page` to *every* command
/// while only eleven actually read them. That made `stats --per-page 3`,
/// `index --per-page 3` and `atlas --per-page 5` return identical line counts:
/// the flag was accepted, validated, and then ignored. This helper exists so a
/// list command gets real paging in one line instead of a bespoke copy.
pub fn page_rows(a: &Args, rows: Vec<String>) -> (Vec<String>, String) {
    // No `--page`/`--per-page` means "give me the whole list".
    //
    // `Page::from_args` defaults to 30 rows, which is right for `list` over
    // 500+ records but catastrophic for a machine-readable inventory: adding
    // paging to `commands` silently cut its output from 70 names to 30 plus a
    // footer, and the docs checker then reported four real commands as
    // nonexistent. Truncating a fixed-size inventory by default is the same
    // class of bug as ignoring a flag -- the caller believes it has the list.
    if !a.has("page") && !a.has("per-page") && !a.has("per") {
        return (rows, String::new());
    }
    let p = Page::from_args(a, rows.len());
    if !p.sliced() {
        return (rows, String::new());
    }
    let slice = rows[p.start()..p.end().min(rows.len())].to_vec();
    (slice, p.bar())
}

/// Slice already-serialised JSON items for the JSON modes of long listings.
///
/// The text path had paging and the JSON path did not: `atlas --json --per-page
/// 3` returned all 42 cards on every "page", so a caller walking pages with a
/// `cards.is_empty()` stop condition looped forever and one that trusted the
/// page count silently re-read the same rows. Machine-readable output is where
/// paging matters most, because a script cannot eyeball a footer.
///
/// Returns the items for this page plus the paging fields to splice into the
/// document. `--limit` is honoured as a hard cap on top of paging so existing
/// callers keep working.
pub fn page_json(a: &Args, items: Vec<String>) -> (Vec<String>, String) {
    let lim = a.num("limit").map(|n| n.max(1) as usize);
    let items = match lim {
        Some(n) if n < items.len() => items[..n].to_vec(),
        _ => items,
    };
    let total = items.len();
    let paged = a.has("page") || a.has("per-page") || a.has("per");
    if !paged {
        // Unpaged: report the honest count so a reader can tell "all of it"
        // from "a slice", without pretending to paginate.
        return (
            items,
            format!("\"page\":1,\"per_page\":{total},\"pages\":1"),
        );
    }
    let mut p = Page::from_args(a, total);
    // Out-of-range pages must come back empty, not clamped to the last page.
    //
    // `Page::from_args` clamps for the text path, which is right for a human
    // who typos `--page 99`: showing the last page beats an error. It is wrong
    // for a machine. A script that walks pages until one comes back empty
    // (`while cards != []`) never sees an empty page under clamping, so it
    // loops forever re-reading the last page. Observed: a 42-card walk
    // accumulated 92 rows, all of them the final page repeated.
    let asked = a.num("page").unwrap_or(1).max(1) as usize;
    let overshot = asked > p.pages;
    if overshot {
        p.page = asked;
    }
    let lo = if overshot { total } else { p.start().min(total) };
    let hi = if overshot { total } else { p.end().min(total) };
    let slice = items[lo..hi].to_vec();
    (slice, p.json_page_only())
}
