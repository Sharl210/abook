//! Uniform result reporting for commands that change the library.
//!
//! `--json` used to be advertised on write commands and honoured on none of
//! them: `abook set --json` printed `SESSION= x` on stdout and put its file
//! list on stderr, so a caller parsing stdout received prose. The read side had
//! already been fixed one command at a time; the write side had not.
//!
//! Two audiences read the same operation. A person gets the short sentence they
//! got before. A caller gets one parseable object saying what happened, what
//! moved and what the exit code means. Both are produced here, so a command
//! cannot advertise one shape and deliver the other.
//!
//! Failures are reported by `report_error` below, which `main` calls, so an
//! error is machine-readable too. A caller that has to string-match
//! `BLOCKED: 正式库修改需要用户明确批准` is back to parsing prose.

use crate::util::{jstr, Args};

/// Report a failure in whichever shape the caller asked for.
///
/// `tag` is the same string the human form prints (`BLOCKED`, `USAGE`,
/// `NOT_FOUND`, `INTEGRITY`, `IO`, `ERROR`) and `code` is what the process will
/// exit with, so a caller can branch on either without parsing the message.
pub fn report_error(a: &Args, tag: &str, msg: &str, code: i32) {
    if a.has("json") {
        println!(
            "{{\"ok\":false,\"error\":{{\"tag\":{},\"message\":{},\"exit\":{}}}}}",
            jstr(tag),
            jstr(msg),
            code
        );
    } else {
        eprintln!("{tag}: {msg}");
    }
}

/// Report the result of a write command as one document.
///
/// `changed` is the file ledger, `output` is everything the command said. Both
/// are facts a caller needs: which files moved, and what the tool reported. The
/// prose stays available instead of being thrown away, so a caller can show the
/// user the same sentence the terminal would have shown.
pub fn report_ok(root: &std::path::Path, changed: &[String], lines: &[String], code: i32) {
    let _ = root;
    let mut o = String::from("{\"ok\":true,\"exit\":");
    o.push_str(&code.to_string());
    o.push_str(",\"changed\":[");
    for (i, c) in changed.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push_str(&jstr(c));
    }
    o.push_str("],\"output\":[");
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push_str(&jstr(l));
    }
    o.push_str("]}\n");
    print!("{o}");
}

/// A failure that happened after some lines were already collected.
///
/// Both halves matter: the caller learns the operation failed *and* sees how far
/// it got. Dropping the partial output would hide which file was being written
/// when it stopped.
pub fn report_error_lines(a: &Args, tag: &str, msg: &str, code: i32, lines: &[String]) {
    let _ = a;
    let mut o = String::from("{\"ok\":false,\"error\":{\"tag\":");
    o.push_str(&jstr(tag));
    o.push_str(",\"message\":");
    o.push_str(&jstr(msg));
    o.push_str(",\"exit\":");
    o.push_str(&code.to_string());
    o.push_str("},\"output\":[");
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push_str(&jstr(l));
    }
    o.push_str("]}\n");
    print!("{o}");
}
