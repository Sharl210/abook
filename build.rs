//! Stamp the release binary with a fingerprint of the sources it was built
//! from.
//!
//! Why this exists: every earlier attempt to answer "is this binary actually
//! built from these sources?" compared mtimes. That is unreliable in both
//! directions. `cp` a file and `mv` it back and cargo sees the old timestamp,
//! decides nothing changed, and skips the rebuild -- the stale binary then runs
//! every later check and reports success. The reverse also bites: after one
//! manual `cargo build`, the next `cargo build` has nothing to do and prints
//! "Finished" without "Compiling", which the old check read as a stale binary.
//!
//! So instead of guessing, the binary now carries the answer. FNV-1a over every
//! `src/**/*.rs` in sorted path order, embedded as a compile-time env var. The
//! preflight recomputes the same hash from the working tree and compares it
//! with what the binary reports. A mismatch means the binary is not what the
//! sources say it is, with no mtime involved.
//!
//! FNV rather than SHA-256: this is a consistency check against accidental
//! drift, not a security boundary, and it keeps `build.rs` dependency-free like
//! the crate itself.

use std::fs;
use std::path::Path;

fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
            out.push(p);
        }
    }
}

fn main() {
    let mut files = Vec::new();
    collect(Path::new("src"), &mut files);
    files.sort();

    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for f in &files {
        // The path takes part in the hash so that renaming a file counts as a
        // change even when the bytes are identical.
        for b in f.to_string_lossy().as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        if let Ok(data) = fs::read(f) {
            for b in data {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        println!("cargo:rerun-if-changed={}", f.display());
    }
    println!("cargo:rustc-env=ABOOK_SRC_FP={h:016x}");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");

    // 打了新 tag 却没重编，二进制会一直自称上一个版本。
    //
    // 版本号是 `git describe` 算出来的，但 cargo 不知道"tag 列表"是构建输入，
    // 于是只要源码没动它就认为无事可做，直接报 `Finished in 0.00s`，
    // 编出来的还是旧版本号。本地刚踩过：打完 v1.6.10 再 build，`version`
    // 仍然回答 1.6.9，必须先 `touch src/main.rs` 强制重编才对。
    //
    // 把决定版本号的 git 元数据声明成构建依赖，让 cargo 自己发现"该重编了"。
    // 只盯三个小文件/目录，不是整个 `.git`（那个每次操作都在变，会把增量
    // 编译彻底废掉）。三个都不存在时（tarball、浅目录导出）就不声明，
    // 让版本号走 `CARGO_PKG_VERSION` 兜底。
    for p in [".git/HEAD", ".git/packed-refs", ".git/refs/tags"] {
        if Path::new(p).exists() {
            println!("cargo:rerun-if-changed={p}");
        }
    }

    println!("cargo:rustc-env=ABOOK_VERSION={}", version());
}

/// The version this binary announces.
///
/// Why not just `CARGO_PKG_VERSION`: that field only changes when a human
/// remembers to edit it, and nobody did -- the repo shipped tags v1.0.0 through
/// v1.6.1 while `Cargo.toml` still said 1.0.0, so every published binary
/// misreported itself as the first release. `version` could not be used to tell
/// two builds apart, and the release page described a version the download did
/// not claim. Trusting a hand-edited field is the bug; deriving the answer from
/// the tag cannot drift from it.
///
/// Order: the release tag CI checked out, then `git describe`, then
/// `Cargo.toml`. The last one only applies to a tarball with no VCS metadata.
fn version() -> String {
    // CI builds on a tag checkout; GITHUB_REF_NAME is exactly `v1.6.1` there.
    if let Ok(r) = std::env::var("GITHUB_REF_NAME") {
        let v = r.trim().trim_start_matches('v').to_string();
        if is_semverish(&v) {
            return v;
        }
    }
    // Local build: nearest tag, so a laptop build also reports something real.
    // `--dirty` is deliberately absent -- a dirty tree already shows up as a
    // changed `src=` fingerprint, and appending it here would make two builds
    // of the same tag announce different versions.
    if let Ok(out) = std::process::Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .output()
    {
        if out.status.success() {
            let v = String::from_utf8_lossy(&out.stdout)
                .trim()
                .trim_start_matches('v')
                .to_string();
            if is_semverish(&v) {
                return v;
            }
        }
    }
    std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into())
}

/// `1.6.1`, `1.6`, `1` -- starts with a digit, holds only digits and dots.
/// Guards against `git describe` handing back a branch or commit name.
fn is_semverish(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
        && s.chars().all(|c| c.is_ascii_digit() || c == '.')
}
