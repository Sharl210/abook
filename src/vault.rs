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
use std::fs;
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

/// MAC 行的标记词。
///
/// 密文正文里全是 0..=255 的十进制数字，不可能是 `mac`，所以这个词不会
/// 和正文混淆。反过来，旧的二进制读新格式时会把这个词当成"解析不成数字
/// 的 token"直接跳过——见下面 `byte: u16 = match tok.parse() { Err(_) => continue }`
/// ——于是老版本照样能读出正确的明文，只是不做校验。这一点很重要：
/// 加校验不能让任何旧版本读不出新条目。
const MAC_TAG: &str = "mac";

/// 条目校验码。
///
/// 覆盖"种子 + 全部密文数字"，所以任何一个数字被改动、或者密文被截断、
/// 被拼接，都会算不出原来那个值。
///
/// **它防的是什么，必须说清楚**：防的是**损坏**——位翻转、写到一半断电、
/// 文件被别的工具截断、磁盘静默错误。这些是真实发生的事，而原来的实现
/// 对它们完全无感：改一个数字，`verify` 照样报 PASS，`get` 拿到的是被
/// 篡改过的明文。
///
/// 它**防不住**能任意改写这个文件的人：格式自包含的设计要求种子也存在
/// 文件里，而这里没有第二个秘密，所以知道算法的人可以解密、改明文、
/// 重新加密、再把校验码一起算对。要做到真正抗伪造，需要一个不放在库里
/// 的独立密钥，那与"整目录搬走照常可读"的自包含要求直接冲突。两件事
/// 不能同时成立，这里选了自包含 + 损坏检测，并在文档里如实写明边界。
fn entry_mac(seed: &str, cipher_toks: &[&str]) -> String {
    let canon = format!("{}|{}|{}", seed, cipher_toks.len(), cipher_toks.join(" "));
    sha256::digest_prefix(canon.as_bytes(), 16)
}

pub fn decrypt_text(text: &str) -> R<String> {
    decrypt_checked(text).map(|(plain, _)| plain)
}

/// 解密并校验完整性。
///
/// 返回 `(明文, 是否带校验码)`。第二个值让调用方能区分两种"解得出"：
/// 校验通过的，和压根没有校验码的旧条目。旧的实现只有一个字符串返回值，
/// 于是这两件事在调用方看来一模一样，`verify` 只能一律报 PASS。
pub fn decrypt_checked(text: &str) -> R<(String, bool)> {
    let mut lines = text.lines();
    let r = match lines.next() {
        Some(s) => s.trim().to_string(),
        None => return integrity("条目为空"),
    };
    let (m, a) = derive_params(&r)?;
    let seed: u64 = r
        .parse::<u64>()
        .map_err(|_| EE::Integrity("种子不可解析".into()))?;

    // 先把所有 token 摊平，因为 MAC 行可能出现在任意位置（当前实现写在末尾，
    // 但解析不该依赖这一点）。
    let mut toks: Vec<&str> = Vec::new();
    for line in lines {
        for t in line.split_whitespace() {
            toks.push(t);
        }
    }
    let mut expected: Option<String> = None;
    let mut body: Vec<&str> = Vec::with_capacity(toks.len());
    let mut i = 0;
    while i < toks.len() {
        if toks[i] == MAC_TAG && i + 1 < toks.len() {
            expected = Some(toks[i + 1].to_string());
            i += 2;
            continue;
        }
        body.push(toks[i]);
        i += 1;
    }

    let mut s = seed & MASK32;
    let mut out = Vec::new();
    for tok in &body {
        let byte: u16 = match tok.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let k = keystream(&mut s, m, a) as u16;
        out.push(((byte + 256 - k) % 256) as u8);
    }

    // 有校验码就必须对得上。对不上说明文件已经不是当初写下的那一份——
    // 这时候把解出来的明文交出去是危险的，因为它是被篡改后的内容，
    // 而调用方无从分辨。直接报错比给一份错的明文好。
    if let Some(want) = expected {
        let got = entry_mac(&r, &body);
        if got != want {
            return integrity(format!(
                "条目内容校验失败：写下的校验码是 {want}，按当前内容算出的是 {got}。\
                 这说明条目文件在写入之后被改动过（位翻转、截断、被别的工具重写）。\
                 明文不可信，已拒绝返回。若确认这次改动是人为的，重新 vault put 覆盖即可"
            ));
        }
        return Ok((String::from_utf8_lossy(&out).to_string(), true));
    }
    Ok((String::from_utf8_lossy(&out).to_string(), false))
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
    let mut toks: Vec<String> = Vec::with_capacity(plaintext.len());
    for b in plaintext.as_bytes() {
        let k = keystream(&mut s, m, a) as u16;
        toks.push(format!("{}", (*b as u16 + k) % 256));
    }
    for (i, t) in toks.iter().enumerate() {
        out.push_str(t);
        if (i + 1) % 24 == 0 {
            out.push('\n');
        } else {
            out.push(' ');
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    // 校验行放在最后。放在最后是为了让正文的换行节奏只由正文决定，
    // 老版本读到这一行时只是多跳过两个 token，明文完全不受影响。
    let refs: Vec<&str> = toks.iter().map(|s| s.as_str()).collect();
    out.push_str(&format!("{} {}\n", MAC_TAG, entry_mac(r, &refs)));
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
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    // Seeds must not be near-copies of each other. An earlier version derived
    // the main digits from (second, pid) only, so every entry rekeyed in the
    // same second got the SAME first ten digits and differed only in the last
    // two -- all entries then shared almost the same derived parameters.
    // Nanoseconds and a static counter now feed the whole 12 digits.
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let h = sha256::digest_hex(
        format!("{now}-{nanos}-{pid}-{seq}").as_bytes(),
    );
    let a = u64::from_str_radix(&h[0..16], 16).unwrap_or(now);
    let b = u64::from_str_radix(&h[16..24], 16).unwrap_or(nanos);
    let n = a ^ b.rotate_left(17);
    format!("{:05}{:05}{:02}", (n / 100000) % 100000, n % 100000, (n >> 7) % 100)
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub ident: String,
    pub purpose: String,
    pub body: String,
    pub path: PathBuf,
    /// 这个条目是否带完整性校验码。
    ///
    /// 旧格式的条目没有它，`decrypt_checked` 照样解得出明文，但那只是
    /// "读得出来"，不等于"内容没被改过"。`verify` 必须把这两件事分开报，
    /// 否则校验形同虚设——它会对一个无从校验的文件说 PASS。
    pub verified: bool,
}

impl Entry {
}

/// Read one entry by identifier.
pub fn read_entry(root: &Path, ident: &str) -> R<Entry> {
    let p = entry_path(root, ident);
    if !p.exists() {
        return notfound(format!("保险箱无此条目: {ident}"));
    }
    // 读到就顺手收紧权限。
    //
    // 权限只在 write_entry 里设，于是库里已有的旧条目会一直是 644——而它们
    // 恰恰是最该收紧的那批：早期写入的、可能已经跟着备份或压缩包流转过。
    // 等"下次重写时自然变好"是不现实的，重写只在换值或升级格式时发生。
    // 读取是每条凭据高频发生的接触点，在这里补一次代价为零。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(md) = fs::metadata(&p) {
            if md.permissions().mode() & 0o077 != 0 {
                let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o600));
            }
        }
    }
    let raw = read_str(&p)?;
    let (txt, verified) = decrypt_checked(&raw)?;
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
        verified,
    })
}

/// 全部条目（只解出标识与用途，正文不解密）。
///
/// **注意这个函数的语义**：它只返回"读得出"的条目。读不出的（种子坏了、
/// 校验码对不上、文件名不合规）会被跳过——这是列表该有的行为，但绝不能
/// 拿它当自检的依据。`verify` 若遍历这个列表来自检，就会出现"坏文件不在
/// 列表里，于是一个也查不出，最后报 PASS"这种自证清白。自检要用
/// [`scan_vault`]，它把坏文件和好文件一起交出来。
pub fn list_entries(root: &Path) -> Vec<Entry> {
    scan_vault(root).0
}

/// 扫一遍保险箱，把**读得出的**和**读不出的**一起交出来。
///
/// 第二个返回值是 `(文件名, 原因)`。有了它，`verify` 才能如实报告
/// "磁盘上有 4 个文件，其中 1 个解不开"，而不是只报它自己解析成功的那 3 个。
pub fn scan_vault(root: &Path) -> (Vec<Entry>, Vec<(String, String)>) {
    let dir = vault_dir(root);
    let mut good = Vec::new();
    let mut bad = Vec::new();
    let rd = match std::fs::read_dir(&dir) {
        Ok(r) => r,
        // 目录不存在或读不到，是"一个条目都没有"还是"保险箱坏了"？
        // 调用方需要区分，所以这里不返回空列表了事——由 verify 那边
        // 单独判断目录是否存在。
        Err(_) => return (good, bad),
    };
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
        let raw = match read_str(&p) {
            Ok(r) => r,
            Err(e) => {
                bad.push((name, format!("文件读不了: {}", e.msg())));
                continue;
            }
        };
        match decrypt_checked(&raw) {
            Ok((txt, verified)) => {
                let head = txt.lines().next().unwrap_or("").to_string();
                let (id, purpose) = match head.split_once('|') {
                    Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
                    None => (head.trim().to_string(), String::new()),
                };
                if id.is_empty() {
                    bad.push((name, "解出来但没有标识".to_string()));
                    continue;
                }
                good.push(Entry {
                    ident: id,
                    purpose,
                    body: String::new(),
                    path: p,
                    verified,
                });
            }
            // 这里必须收进 bad，不能像以前那样 `if let Ok` 一吞了之。
            Err(e) => bad.push((name, e.msg().to_string())),
        }
    }
    good.sort_by(|a, b| a.ident.cmp(&b.ident));
    bad.sort_by(|a, b| a.0.cmp(&b.0));
    (good, bad)
}

/// Write one entry; verifies by decrypting back and comparing.
/// 目录权限收到 0700（仅属主可进）。
///
/// 失败不报错：非 Unix 平台没有这个概念，某些文件系统（FAT、部分网络盘）
/// 也不支持改权限。能设就设，设不了不该让写入失败——那会让工具在那些环境
/// 上完全不能用，而它本来只是加固。
#[cfg(unix)]
fn restrict_dir(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o700));
}
#[cfg(not(unix))]
fn restrict_dir(_p: &Path) {}

/// 条目文件权限收到 0600（仅属主可读写）。
#[cfg(unix)]
fn restrict_file(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn restrict_file(_p: &Path) {}

pub fn write_entry(root: &Path, ident: &str, purpose: &str, body: &str) -> R<PathBuf> {
    if ident.contains('\'') || ident.contains('\n') {
        return usage("标识不得包含单引号或换行");
    }
    if purpose.contains('\n') {
        return usage("用途不得包含换行");
    }
    // 权限要收紧。密文里的种子就在文件第一行，同机任何用户只要能读这个
    // 文件就能解出全部明文——所以文件权限是这套方案里唯一真正起作用的
    // 访问控制。默认的 755 目录 + 644 文件等于把凭据摊开给所有本地用户。
    //
    // 0700 / 0600 是这套设计能给出的最大边界：它挡的是"同机其他用户与
    // 后台进程"，挡不住能以你的身份运行的程序。
    ensure_dir(&vault_dir(root))?;
    restrict_dir(&vault_dir(root));
    let plain = if body.is_empty() {
        format!("{ident}|{purpose}")
    } else {
        format!("{ident}|{purpose}\n{body}")
    };
    let r = new_seed();
    let enc = encrypt_text(&r, &plain)?;
    let p = entry_path(root, ident);
    write_atomic(&p, &enc)?;
    restrict_file(&p);
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
/// 名册与保险箱是否已经分叉——纯读，不写盘。
///
/// 抽出来是为了让 `--dry-run` 能回答"要不要重建"而不动库：写路径和预演
/// 必须共用同一套判定，否则预演说的和真做的是两件事。
pub fn diff_ledger(root: &Path) -> R<(bool, usize)> {
    let entries = list_entries(root);
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
    Ok((need, others + 1))
}

pub fn sync_ledger(root: &Path) -> R<(bool, usize)> {
    let (need, total) = diff_ledger(root)?;
    if need {
        let want = ledger_body(root);
        write_entry(root, LEDGER_ID, "凭据名册：逐行列出库内各条目的标识与用途，不含任何凭据值", want.trim_end())?;
    }
    Ok((need, total))
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
