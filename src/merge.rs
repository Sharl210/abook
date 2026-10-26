//! Three-way merge for record files.
//!
//! A memory record is a line-oriented markdown file: a header of `KEY=value`
//! lines, a body between tags, and an attachment list. Two writers touching
//! *different* lines is the common case -- one process sets `UPDATED`, another
//! appends a paragraph -- and merging line ranges handles that exactly.
//!
//! The alternative to merging is refusing: `write_atomic_cas` already refuses to
//! overwrite bytes it did not read. But refusing a whole edit because an unrelated
//! line moved is user-hostile when the two changes provably do not interact.
//! Merging is only allowed when the changed line ranges are disjoint; when they
//! overlap the merge fails and the caller reports both versions, so no change is
//! ever silently dropped.

use crate::util::{Conflict, R};

/// One contiguous change: replace `base[start..start+len]` with `lines`.
#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    pub start: usize,
    pub len: usize,
    pub lines: Vec<String>,
}

fn split_lines(s: &str) -> Vec<String> {
    s.lines().map(|l| l.to_string()).collect()
}

/// Longest-common-subsequence table, trimmed to the region that differs.
///
/// Records are at most a few hundred lines, so the quadratic table is cheaper
/// than the bookkeeping of a smarter algorithm -- and far easier to trust.
fn lcs(a: &[String], b: &[String]) -> Vec<Vec<u32>> {
    let mut t = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            t[i][j] = if a[i] == b[j] {
                t[i + 1][j + 1] + 1
            } else {
                t[i + 1][j].max(t[i][j + 1])
            };
        }
    }
    t
}

/// Line-ranged diff: what has to change in `base` to reach `other`.
pub fn diff(base: &str, other: &str) -> Vec<Hunk> {
    let a = split_lines(base);
    let b = split_lines(other);
    let t = lcs(&a, &b);
    let (mut i, mut j) = (0usize, 0usize);
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut cur: Option<Hunk> = None;
    let mut flush = |h: &mut Option<Hunk>, out: &mut Vec<Hunk>| {
        if let Some(x) = h.take() {
            out.push(x);
        }
    };
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            flush(&mut cur, &mut hunks);
            i += 1;
            j += 1;
            continue;
        }
        let take_b = if i >= a.len() {
            true
        } else if j >= b.len() {
            false
        } else {
            t[i + 1][j] >= t[i][j + 1]
        };
        if take_b {
            // `a[i]` is not part of the common subsequence, so it is deleted from
            // the base. Advancing `j` here instead produced hunks that replaced
            // unchanged trailing lines, which made every edit look like it touched
            // the rest of the file.
            let h = cur.get_or_insert(Hunk {
                start: i,
                len: 0,
                lines: Vec::new(),
            });
            h.len += 1;
            i += 1;
        } else {
            let h = cur.get_or_insert(Hunk {
                start: i,
                len: 0,
                lines: Vec::new(),
            });
            h.lines.push(b[j].clone());
            j += 1;
        }
    }
    flush(&mut cur, &mut hunks);
    hunks
}

/// Do two hunks touch overlapping base lines?
///
/// A pure insertion (`len == 0`) at the boundary of another hunk is treated as
/// disjoint: appending after a line and rewriting that same line do not conflict.
fn overlaps(a: &Hunk, b: &Hunk) -> bool {
    if a.len == 0 && b.len == 0 {
        return a.start == b.start && a.lines != b.lines;
    }
    if a.len == 0 {
        return a.start > b.start && a.start < b.start + b.len;
    }
    if b.len == 0 {
        return b.start > a.start && b.start < a.start + a.len;
    }
    a.start < b.start + b.len && b.start < a.start + a.len
}

/// Apply both sides' changes to `base`. Fails when they touch the same lines.
pub fn merge3(base: &str, ours: &str, theirs: &str) -> R<String> {
    let ho = diff(base, ours);
    let ht = diff(base, theirs);
    let base_lines = split_lines(base);

    // Collect every change, ordered by where it starts in `base`. When both sides
    // made the identical change, one copy is enough.
    let mut all: Vec<(Hunk, bool)> = Vec::new(); // (hunk, from_theirs)
    for h in &ho {
        all.push((h.clone(), false));
    }
    for h in &ht {
        if ho.iter().any(|o| o == h) {
            continue;
        }
        all.push((h.clone(), true));
    }
    all.sort_by_key(|(h, from_theirs)| (h.start, *from_theirs));
    for i in 0..all.len() {
        for j in (i + 1)..all.len() {
            if overlaps(&all[i].0, &all[j].0) && all[i].0 != all[j].0 {
                return Err(crate::util::EE::IntegrityConflict(Conflict::build(
                    format!("第 {} 行附近", all[i].0.start + 1),
                    base.to_string(),
                    ours.to_string(),
                    theirs.to_string(),
                )));
            }
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut pos = 0usize;
    for (h, _) in &all {
        while pos < h.start {
            out.push(base_lines[pos].clone());
            pos += 1;
        }
        out.extend(h.lines.iter().cloned());
        pos = h.start + h.len;
    }
    while pos < base_lines.len() {
        out.push(base_lines[pos].clone());
        pos += 1;
    }
    let mut s = out.join("\n");
    if base.ends_with('\n') || ours.ends_with('\n') || theirs.ends_with('\n') {
        s.push('\n');
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_edits_merge() {
        let base = "A=1\nB=2\nC=3\n";
        let ours = "A=1\nB=99\nC=3\n";
        let theirs = "A=7\nB=2\nC=3\n";
        let m = merge3(base, ours, theirs).unwrap();
        assert_eq!(m, "A=7\nB=99\nC=3\n");
    }

    #[test]
    fn same_line_different_value_conflicts() {
        let base = "A=1\nB=2\n";
        let ours = "A=1\nB=99\n";
        let theirs = "A=1\nB=77\n";
        assert!(merge3(base, ours, theirs).is_err());
    }

    #[test]
    fn identical_change_applies_once() {
        let base = "A=1\nB=2\n";
        let ours = "A=1\nB=9\n";
        assert_eq!(merge3(base, ours, ours).unwrap(), "A=1\nB=9\n");
    }

    #[test]
    fn append_and_edit_merge() {
        let base = "A=1\nB=2\n";
        let ours = "A=1\nB=2\nnew tail\n";
        let theirs = "A=5\nB=2\n";
        let m = merge3(base, ours, theirs).unwrap();
        assert_eq!(m, "A=5\nB=2\nnew tail\n");
    }

    #[test]
    fn deletion_and_unrelated_edit_merge() {
        let base = "A=1\nB=2\nC=3\n";
        let ours = "A=1\nC=3\n";
        let theirs = "A=1\nB=2\nC=9\n";
        let m = merge3(base, ours, theirs).unwrap();
        assert_eq!(m, "A=1\nC=9\n");
    }
}
