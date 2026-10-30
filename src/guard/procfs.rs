//! Linux / Android 实现：走 `/proc`。
//!
//! 管道在 fd 上就是一个 `pipe:[N]` 的符号链接，持有者可以从 `/proc/<pid>/fd`
//! 反查出来，改道就是 `dup2`。Android 与 Linux 共用这一份：两者都有完整的
//! `/proc`，`dup2` 语义相同。实测 Android aarch64 可用。
//!
//! ## 为什么每次都要全扫
//!
//! 想过加"父进程命令行里没有截断词就跳过"的快速闸门，但那个闸门是错的：
//! `abook list | head -3` 里 `head` 是**兄弟**进程，不是父进程，父进程命令行
//! 里只看得到 `bash -c "abook list | head -3"` 这一段——它确实含 `head`，但
//! 含 `head` 也不代表真在截断（脚本里提一句不算）。用命令行猜会既误报又漏报。
//!
//! 真正省下来的地方在别处：`stdout` 不是管道时（写文件、写 socket、直接
//! 输出到终端）`stdout_pipe()` 返回 `None`，一次 `readlink` 就结束了，根本不
//! 进入扫描。实测 `abook version` 的开销差异落在噪声里。
//!
//! 真的进入扫描时（stdout 是管道）本机 8 个进程约 1.1 ms，对 `list` 的
//! 210 ms 无感。

use crate::guard::{truncator_of, Support};
use std::fs;

extern "C" {
    fn dup2(oldfd: i32, newfd: i32) -> i32;
}

/// 沿管道链向上找截断程序，最多 `MAX_HOPS` 跳。
///
/// 链式写法 `abook list | grep x | head -3` 里，我们 stdout 管道的读端在 `grep`
/// 手里，不在 `head` 手里。只看第一跳会判定"没人截断"而放过——实测这种写法
/// 54 条只剩 3 条，而且有过滤器作掩护，比直接 `| head` 更隐蔽。
///
/// 走法：当前管道 → 谁持有它 → 那些进程往哪个管道写 → 继续。链条在遇到
/// socket/文件（数据已离开管道）或没有下一跳时自然终止。
const MAX_HOPS: usize = 4;

pub fn support() -> Support {
    Support {
        os: if cfg!(target_os = "android") {
            "android"
        } else {
            "linux"
        },
        functional: true,
        detail: "读 /proc/<pid>/fd 找管道持有者，dup2 改道",
    }
}

fn readlink(pid: &str, fd: &str) -> Option<String> {
    fs::read_link(format!("/proc/{pid}/fd/{fd}"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

pub fn stdout_pipe() -> Option<String> {
    let p = fs::read_link("/proc/self/fd/1").ok()?;
    let s = p.to_string_lossy().into_owned();
    s.starts_with("pipe:").then_some(s)
}

pub fn stderr_is_stdout() -> bool {
    match (readlink("self", "1"), readlink("self", "2")) {
        (Some(a), Some(b)) => a == b,
        // 读不到就当作同一路：宁可只报警告，也不要在不确定时乱改道。
        _ => true,
    }
}

pub fn redirect_stdout_to_stderr() -> bool {
    unsafe { dup2(2, 1) >= 0 }
}

fn cmdline_of(pid: &str) -> Option<Vec<String>> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    if raw.is_empty() {
        return None;
    }
    Some(
        raw.split(|c| *c == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect(),
    )
}

pub fn find_truncator(first_pipe: &str, me: &str) -> Option<String> {
    let mut cur = first_pipe.to_string();
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..MAX_HOPS {
        if seen.iter().any(|p| *p == cur) {
            return None;
        }
        seen.push(cur.clone());

        // 谁持有当前管道（排除自己：我们持有的是写端）
        let mut holders: Vec<(String, Vec<String>)> = Vec::new();
        let rd = fs::read_dir("/proc").ok()?;
        for ent in rd.flatten() {
            let pid = ent.file_name().to_string_lossy().into_owned();
            if !pid.bytes().all(|c| c.is_ascii_digit()) || pid == me {
                continue;
            }
            let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else {
                continue;
            };
            let holds = fds.flatten().any(|fd| {
                fs::read_link(fd.path())
                    .map(|p| p.to_string_lossy() == cur)
                    .unwrap_or(false)
            });
            if !holds {
                continue;
            }
            if let Some(argv) = cmdline_of(&pid) {
                holders.push((pid, argv));
            }
        }
        if holders.is_empty() {
            return None;
        }
        if std::env::var_os("ABOOK_GUARD_DEBUG").is_some() {
            for (pid, argv) in &holders {
                eprintln!(
                    "[abook guard] 持有者 pid={pid} argv={argv:?} 判定={:?}",
                    truncator_of(argv)
                );
            }
        }
        // 这一跳里有截断程序就直接命中
        for (_, argv) in &holders {
            if let Some(t) = truncator_of(argv) {
                return Some(t);
            }
        }
        // 否则顺着它们的 stdout 往上走一跳
        let next = holders
            .iter()
            .filter_map(|(pid, _)| readlink(pid, "1"))
            .find(|s| s.starts_with("pipe:"));
        match next {
            Some(n) => cur = n,
            None => return None,
        }
    }
    None
}
