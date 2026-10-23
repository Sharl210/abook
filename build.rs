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
}
