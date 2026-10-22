//! Credential vault: derivable entry names, stream cipher, ledger.
//!
//! Entry file layout (no extension, opaque name):
//!   line 1        : 12-digit session seed R
//!   line 2..n     : decimal byte stream, whitespace separated
//!
//! Plaintext payload layout:
//!   line 1        : <ident>|<purpose>
//!   line 2..n     : credential body
//!
//! Parameters are derived from R alone, so an entry is self-contained:
//! nothing depends on filename, path, mtime or permissions.

use crate::sha256;
use crate::util::*;
use std::path::{Path, PathBuf};

/// Substitution table used to fold seed digits into parameter choices.
const SUB: &[u8; 10] = b"4729618035";

/// 24 rows of (m0, a0).
const TBL: [(u64, u64); 24] = [
    (1250495, 361194364),
    (1328397, 164672004),
    (1074760, 141616624),
    (1519164, 317151836),
    (1378904, 157351270),
    (1326088, 138005596),
    (1194028, 349965171),
    (1306704, 236393770),
    (1007402, 171566145),
    (1368302, 389074981),
    (1205810, 106555540),
    (1529001, 176487109),
    (1546030, 137615837),
    (1296091, 209823580),
    (1448482, 160415404),
    (1444499, 163306626),
    (1057533, 119090339),
    (1424386, 119988298),
    (1133948, 239066141),
    (1331891, 115345966),
    (1192472, 364824958),
    (1144647, 414890006),
    (1137794, 169795202),
    (1110686, 215549745),
];

/// Naming salt for entry filenames. Kept as an arithmetic expression so the
/// value is not a single literal at call sites.
fn naming_salt() -> u64 {
    1732_984_557
}

/// Ledger entry identifier.
pub const LEDGER_ID: &str = "aq7x0k3m9p";

const MASK32: u64 = 0xFFFF_FFFF;

/// Derive (m, a) from the 12-digit seed.
pub fn derive_params(r: &str) -> R<(u64, u64)> {
    let d: Vec<usize> = r.chars().map(|c| c as usize - '0' as usize).collect();
    if d.len() != 12 || d.iter().any(|&x| x > 9) {
        return integrity(format!("会话种子必须为 12 位数字，实际 {}", r));
    }
    let e: Vec<u64> = [d[2], d[6], d[10]]
        .iter()
        .map(|&i| (SUB[i] - b'0') as u64)
        .collect();
    let mut k = (e[0] + 2 * e[1] + 4 * e[2]) % 24;
    if k >= 24 {
        k %= 24;
    }
    let u = 100 * e[0] + 10 * e[1] + e[2];
    let (m0, a0) = TBL[k as usize];
    Ok((m0 + u, a0 + 10_000 * u))
}

fn keystream(state: &mut u64, m: u64, a: u64) -> u8 {
    *state = (m * *state + a) & MASK32;
    ((*state >> 16) & 0xFF) as u8
}

pub fn decrypt_text(text: &str) -> R<String> {
    let mut lines = text.lines();
    let r = match lines.next() {
        Some(s) => s.trim().to_string(),
        None => return integrity("条目为空"),
    };
    let (m, a) = derive_params(&r)?;
    let seed: u64 = r
        .parse::<u64>()
        .map_err(|_| EE::Integrity("种子不可解析".into()))?;
    let mut s = seed & MASK32;
    let mut out = Vec::new();
    for line in lines {
        for tok in line.split_whitespace() {
            let byte: u16 = match tok.parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let k = keystream(&mut s, m, a) as u16;
            out.push(((byte + 256 - k) % 256) as u8);
        }
    }
    Ok(String::from_utf8_lossy(&out).to_string())
}

pub fn encrypt_text(r: &str, plaintext: &str) -> R<String> {
    let (m, a) = derive_params(r)?;
    let seed: u64 = r
        .parse::<u64>()
        .map_err(|_| EE::Integrity("种子不可解析".into()))?;
    let mut s = seed & MASK32;
    let mut out = String::new();
    out.push_str(r);
    out.push('\n');
    let mut col = 0;
    for b in plaintext.as_bytes() {
        let k = keystream(&mut s, m, a) as u16;
        out.push_str(&format!("{}", (*b as u16 + k) % 256));
        col += 1;
        if col % 24 == 0 {
            out.push('\n');
        } else {
            out.push(' ');
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// Entry filename: first 16 hex chars of SHA-256 over ident+salt.
pub fn entry_name(ident: &str) -> String {
    let salt = naming_salt();
    sha256::digest_prefix(format!("{ident}{salt}").as_bytes(), 16)
}

pub fn vault_dir(root: &Path) -> PathBuf {
    root.join("vault")
}

pub fn entry_path(root: &Path, ident: &str) -> PathBuf {
    vault_dir(root).join(entry_name(ident))
}

/// Generate a 12-digit seed string.
pub fn new_seed() -> String {
    let now = now_secs() as u64;
    let pid = std::process::id() as u64;
    let mut h = sha256::digest_hex(format!("{now}-{pid}-{}", now_secs()).as_bytes());
    // mix in a monotonic-ish counter so two calls in one second differ
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    h.push_str(&sha256::digest_hex(format!("{nanos}-{pid}").as_bytes()));
    let n = u64::from_str_radix(&h[6..14], 16).unwrap_or(now);
    format!("{:05}{:05}{:02}", (n / 100000) % 100000, n % 100000, nanos % 100)
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub ident: String,
    pub purpose: String,
    pub body: String,
    pub path: PathBuf,
}

impl Entry {
    pub fn plaintext(&self) -> String {
        if self.body.is_empty() {
            format!("{}|{}", self.ident, self.purpose)
        } else {
            format!("{}|{}\n{}", self.ident, self.purpose, self.body)
        }
    }
}

/// Read one entry by identifier.
pub fn read_entry(root: &Path, ident: &str) -> R<Entry> {
    let p = entry_path(root, ident);
    if !p.exists() {
        return notfound(format!("保险箱无此条目: {ident}"));
    }
    let raw = read_str(&p)?;
    let txt = decrypt_text(&raw)?;
    let mut it = txt.splitn(2, '\n');
    let head = it.next().unwrap_or("").to_string();
    let body = it.next().unwrap_or("").to_string();
    let (id, purpose) = match head.split_once('|') {
        Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
        None => (head.trim().to_string(), String::new()),
    };
    Ok(Entry {
        ident: id,
        purpose,
        body: body.trim_end_matches('\n').to_string(),
        path: p,
    })
}

/// All entries (identifier + purpose only; body is not decrypted for listing).
pub fn list_entries(root: &Path) -> Vec<Entry> {
    let dir = vault_dir(root);
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.starts_with('.') || name.len() > 32 {
                continue;
            }
            if let Ok(raw) = read_str(&p) {
                if let Ok(txt) = decrypt_text(&raw) {
                    let head = txt.lines().next().unwrap_or("").to_string();
                    let (id, purpose) = match head.split_once('|') {
                        Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
                        None => (head.trim().to_string(), String::new()),
                    };
                    if !id.is_empty() {
                        v.push(Entry {
                            ident: id,
                            purpose,
                            body: String::new(),
                            path: p,
                        });
                    }
                }
            }
        }
    }
    v.sort_by(|a, b| a.ident.cmp(&b.ident));
    v
}

/// Write one entry; verifies by decrypting back and comparing.
pub fn write_entry(root: &Path, ident: &str, purpose: &str, body: &str) -> R<PathBuf> {
    if ident.contains('\'') || ident.contains('\n') {
        return usage("标识不得包含单引号或换行");
    }
    if purpose.contains('\n') {
        return usage("用途不得包含换行");
    }
    ensure_dir(&vault_dir(root))?;
    let plain = if body.is_empty() {
        format!("{ident}|{purpose}")
    } else {
        format!("{ident}|{purpose}\n{body}")
    };
    let r = new_seed();
    let enc = encrypt_text(&r, &plain)?;
    let p = entry_path(root, ident);
    write_atomic(&p, &enc)?;
    // read-back verification
    let back = decrypt_text(&read_str(&p)?)?;
    if back.trim_end() != plain.trim_end() {
        return integrity(format!("保险箱写入自检失败: {ident}"));
    }
    Ok(p)
}

pub fn delete_entry(root: &Path, ident: &str) -> R<bool> {
    let p = entry_path(root, ident);
    if !p.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&p)?;
    Ok(true)
}

/// Ledger body: one `ident|purpose` line per vault entry.
pub fn ledger_body(root: &Path) -> String {
    let entries = list_entries(root);
    let mut s = String::new();
    for e in &entries {
        if e.ident == LEDGER_ID {
            continue;
        }
        s.push_str(&format!("{}|{}\n", e.ident, e.purpose));
    }
    // the ledger lists itself as well
    s.push_str(&format!(
        "{}|凭据名册：逐行列出库内各条目的标识与用途，不含任何凭据值\n",
        LEDGER_ID
    ));
    s
}

/// Sync the ledger entry with the actual directory contents.
pub fn sync_ledger(root: &Path) -> R<(bool, usize)> {
    let entries = list_entries(root);
    // count real files excluding the ledger itself
    let others = entries.iter().filter(|e| e.ident != LEDGER_ID).count();
    let want = ledger_body(root);
    let cur = read_entry(root, LEDGER_ID).ok();
    let need = match &cur {
        None => true,
        Some(e) => {
            let mut have = String::new();
            for line in e.body.lines() {
                if !line.trim().is_empty() {
                    have.push_str(line.trim());
                    have.push('\n');
                }
            }
            have != want
        }
    };
    if need {
        write_entry(root, LEDGER_ID, "凭据名册：逐行列出库内各条目的标识与用途，不含任何凭据值", want.trim_end())?;
    }
    Ok((need, others + 1))
}

/// Vault accounting: files on disk vs ledger rows.
pub fn reconcile(root: &Path) -> (usize, usize, Vec<String>) {
    let dir = vault_dir(root);
    let mut files = 0usize;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.path().is_file() {
                files += 1;
            }
        }
    }
    let mut problems = Vec::new();
    let mut rows = 0usize;
    match read_entry(root, LEDGER_ID) {
        Ok(led) => {
            let listed: Vec<String> = led
                .body
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.split('|').next().unwrap_or("").trim().to_string())
                .filter(|x| !x.is_empty())
                .collect();
            rows = listed.len();
            let actual: Vec<String> = list_entries(root).into_iter().map(|e| e.ident).collect();
            for a in &actual {
                if !listed.contains(a) {
                    problems.push(format!("名册缺行: {a}"));
                }
            }
            for l in &listed {
                if !actual.contains(l) {
                    problems.push(format!("名册有多余行: {l}"));
                }
            }
            if files != rows {
                problems.push(format!("文件数 {files} != 名册行数 {rows}"));
            }
        }
        Err(_) => problems.push("名册缺失或不可解".into()),
    }
    (files, rows, problems)
}
