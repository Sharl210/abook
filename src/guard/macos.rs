//! macOS 实现：走 `libproc`。
//!
//! macOS 没有 `/proc`，等价信息在 libproc 里：
//!
//! | 要做的事 | Linux | macOS |
//! |---|---|---|
//! | 枚举进程 | `readdir("/proc")` | `proc_listpids(PROC_ALL_PIDS, …)` |
//! | 枚举某进程的 fd | `readdir("/proc/<pid>/fd")` | `proc_pidinfo(pid, PROC_PIDLISTFDS, …)` |
//! | fd 指向哪个管道 | `readlink` 得到 `pipe:[N]` | `proc_pidfdinfo(pid, fd, PROC_PIDFDPIPEINFO, …)` 得到 `pipe_handle` |
//! | 进程名 | 读 `/proc/<pid>/cmdline` | `proc_pidpath(pid, …)` |
//!
//! ## 为什么也要查自己的 fd 1
//!
//! 最省事的写法是对自己的 fd 1 做 `fstat` 取 `st_ino`（macOS 上管道句柄就是它）。
//! 但那要手工声明 `struct stat`，而它的字段顺序随平台和 ABI 变化——写错一个
//! 偏移就会读到垃圾值，比不实现更糟。同一件事用 `proc_pidfdinfo` 查自己的
//! fd 1 就能拿到 `pipe_handle`，走的是同一条已验证的路径，不必碰布局。
//!
//! ## 匹配判据
//!
//! 管道有两端，各自有独立句柄，且互相记着对方的 `pipe_peer_handle`。所以判断
//! "这个进程持有我们的管道"看两个条件之一即可：它的 `pipe_handle` 等于我们的，
//! 或者它的 `pipe_peer_handle` 等于我们的。后者正是 `abook | head` 里 head 的
//! 情形——它持有读端，记着我们的写端句柄。

use crate::guard::{truncator_of, Support};
use std::ffi::CStr;

// libproc 接口。签名照 <libproc.h> 原样声明。
//
// 不需要写 `#[link]`：这些函数都在 libSystem 里，而 libSystem 是 macOS 上
// rustc 默认链接的那一个库。写 `#[link(name = "libproc")]` 反而会失败——
// 系统里没有独立的 libproc.dylib，它是 <libproc.h> 声明、libSystem 实现。
extern "C" {
    fn proc_listpids(type_: u32, typeinfo: u32, buffer: *mut u8, buffersize: i32) -> i32;
    fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut u8, buffersize: i32) -> i32;
    fn proc_pidfdinfo(pid: i32, fd: i32, flavor: i32, buffer: *mut u8, buffersize: i32) -> i32;
    fn proc_pidpath(pid: i32, buffer: *mut u8, buffersize: u32) -> i32;
    fn dup2(oldfd: i32, newfd: i32) -> i32;
    fn getpid() -> i32;
}

const PROC_ALL_PIDS: u32 = 1;
const PROC_PIDLISTFDS: i32 = 1;
const PROC_PIDFDPIPEINFO: i32 = 6;
const PROX_FDTYPE_PIPE: u32 = 6;
/// `<sys/proc_info.h>` 里 `proc_fdinfo` 是 `{int32 fd; uint32 fdtype;}`，8 字节。
const FDINFO_SIZE: usize = 8;
/// `proc_fdinfo` 里 fdtype 的偏移。
const FDINFO_TYPE_OFF: usize = 4;
/// `pipe_fdinfo` 前 24 字节就够用：pipe_handle(8) + file_offset(8) +
/// pipe_status(4) + pipe_peer_status(4)，之后 pipe_peer_handle(8)。
/// 整个结构 32 字节，按 64 字节开缓冲留余量。
const PIPEINFO_BUF: usize = 64;
const PIPEINFO_PEER_OFF: usize = 24;

const MAX_HOPS: usize = 4;

pub fn support() -> Support {
    Support {
        os: "macos",
        functional: true,
        detail: "用 libproc 枚举管道持有者，dup2 改道",
    }
}

fn read_u32(buf: &[u8], off: usize) -> u32 {
    let mut b = [0u8; 4];
    b.copy_from_slice(&buf[off..off + 4]);
    u32::from_le_bytes(b)
}

fn read_u64(buf: &[u8], off: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&buf[off..off + 8]);
    u64::from_le_bytes(b)
}

/// 查某个 (pid, fd) 的管道句柄与对端句柄。不是管道就返回 None。
fn pipe_handles(pid: i32, fd: i32) -> Option<(u64, u64)> {
    let mut buf = [0u8; PIPEINFO_BUF];
    let n = unsafe {
        proc_pidfdinfo(
            pid,
            fd,
            PROC_PIDFDPIPEINFO,
            buf.as_mut_ptr(),
            PIPEINFO_BUF as i32,
        )
    };
    // 返回字节数不足说明这个 fd 不是管道，或调用被拒。
    if n < (PIPEINFO_PEER_OFF + 8) as i32 {
        return None;
    }
    Some((read_u64(&buf[..], 0), read_u64(&buf[..], PIPEINFO_PEER_OFF)))
}

/// 自身 fd 1 若是管道，返回它的句柄。
pub fn stdout_pipe() -> Option<String> {
    let pid = unsafe { getpid() };
    let (h, _) = pipe_handles(pid, 1)?;
    Some(h.to_string())
}

/// 取"该进程往哪个管道写"（fd 1）的句柄，用于顺着链往上走一跳。
fn pid_out_handle(pid: i32) -> Option<u64> {
    pipe_handles(pid, 1).map(|(h, _)| h)
}

fn pid_name(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    CStr::from_bytes_until_nul(&buf)
        .ok()
        .map(|c| c.to_string_lossy().into_owned())
}

fn all_pids() -> Vec<i32> {
    let need = unsafe { proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
    if need <= 0 {
        return Vec::new();
    }
    let cap = need as usize + 4 * 64;
    let mut buf = vec![0u8; cap];
    let n = unsafe { proc_listpids(PROC_ALL_PIDS, 0, buf.as_mut_ptr(), cap as i32) };
    if n <= 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 4 <= n as usize {
        let pid = read_u32(&buf, off) as i32;
        if pid > 0 {
            out.push(pid);
        }
        off += 4;
    }
    out
}

pub fn stderr_is_stdout() -> bool {
    let pid = unsafe { getpid() };
    match (pipe_handles(pid, 1), pipe_handles(pid, 2)) {
        (Some((a, _)), Some((b, _))) => a == b,
        // 有一端不是管道，就不是同一路；两端都读不到时保守当同一路。
        (None, _) | (_, None) => {
            // 一端是管道一端不是 → 不同路，可以放心改道。
            let p1 = pipe_handles(pid, 1).is_some();
            let p2 = pipe_handles(pid, 2).is_some();
            p1 == p2
        }
    }
}

pub fn redirect_stdout_to_stderr() -> bool {
    unsafe { dup2(2, 1) >= 0 }
}

pub fn find_truncator(first_pipe: &str, me: &str) -> Option<String> {
    let me_pid: i32 = me.parse().unwrap_or_else(|_| unsafe { getpid() });
    let Ok(my_handle) = first_pipe.parse::<u64>() else {
        return None;
    };

    let mut cur = my_handle;
    let mut seen: Vec<u64> = Vec::new();
    for _ in 0..MAX_HOPS {
        if seen.contains(&cur) {
            return None;
        }
        seen.push(cur);

        let mut holders: Vec<i32> = Vec::new();
        for pid in all_pids() {
            if pid == me_pid {
                continue;
            }
            let need = unsafe { proc_pidinfo(pid, PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
            if need <= 0 {
                continue;
            }
            let cap = need as usize + FDINFO_SIZE * 8;
            let mut buf = vec![0u8; cap];
            let n = unsafe {
                proc_pidinfo(pid, PROC_PIDLISTFDS, 0, buf.as_mut_ptr(), cap as i32)
            };
            if n <= 0 {
                continue;
            }
            let mut off = 0usize;
            let mut holds = false;
            while off + FDINFO_SIZE <= n as usize {
                let fd = read_u32(&buf, off) as i32;
                let ty = read_u32(&buf, off + FDINFO_TYPE_OFF);
                off += FDINFO_SIZE;
                if ty != PROX_FDTYPE_PIPE {
                    continue;
                }
                if let Some((h, peer)) = pipe_handles(pid, fd) {
                    if h == cur || peer == cur {
                        holds = true;
                        break;
                    }
                }
            }
            if holds {
                holders.push(pid);
            }
        }

        if holders.is_empty() {
            return None;
        }
        for pid in &holders {
            if let Some(name) = pid_name(*pid) {
                // 用与 Linux 同一套判据：basename + 截断程序表。
                if let Some(t) = truncator_of(&[name]) {
                    return Some(t);
                }
            }
        }
        // 顺着它们的 stdout 往上走一跳
        let next = holders.iter().find_map(|pid| pid_out_handle(*pid));
        match next {
            Some(n) => cur = n,
            None => return None,
        }
    }
    None
}
