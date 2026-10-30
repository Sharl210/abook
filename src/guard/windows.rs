//! Windows 实现：枚举系统句柄表，找出谁持有我们的管道。
//!
//! Windows 没有 `/proc`，也没有 fd 符号链接。等价做法是问内核要"所有进程的
//! 所有句柄"，逐个复制到本进程，再用 `CompareObjectHandles` 判断是不是同一个
//! 管道对象。这是唯一能在不改动其他进程的前提下拿到答案的路径。
//!
//! | 要做的事 | Linux | Windows |
//! |---|---|---|
//! | stdout 是不是管道 | `readlink("/proc/self/fd/1")` | `GetFileType(GetStdHandle(STD_OUTPUT_HANDLE))` |
//! | 谁持有这个管道 | 扫 `/proc/*/fd` 找同一 `pipe:[N]` | `NtQuerySystemInformation` 全表 + `CompareObjectHandles` |
//! | 进程名 | `/proc/<pid>/cmdline` | `QueryFullProcessImageNameW` |
//! | 改道 | `dup2(2,1)` | `SetStdHandle(STD_OUTPUT_HANDLE, stderr)` |
//!
//! ## 已知限制（诚实记录，不假装没有）
//!
//! **不做链式追踪。** Linux 那边能顺着 `abook | grep x | head -3` 查到第二跳，
//! 因为持有者自己的 fd 1 是可以读的。Windows 的句柄表只说"某进程持有某对象"，
//! 不说"这是它的 stdout"，所以拿不到下一跳。Windows 上只能拦住直接写在
//! 管道后面的那个截断程序。这个差异不影响 `abook | more` 这类常见写法。
//!
//! **可能被权限拒绝。** 复制别的进程的句柄需要 `PROCESS_DUP_HANDLE`，跨用户或
//! 受保护进程会被拒。被拒就跳过——宁可漏一个可疑进程，也不能因为查不到就
//! 胡乱改道。

use crate::guard::{truncator_of, Support};
use std::ffi::c_void;

type Handle = *mut c_void;
type NtStatus = i32;

const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
const STD_ERROR_HANDLE: u32 = -12i32 as u32;
const FILE_TYPE_PIPE: u32 = 3;
const STATUS_INFO_LENGTH_MISMATCH: NtStatus = 0xC000_0004u32 as i32;
const SYSTEM_EXTENDED_HANDLE_INFORMATION: u32 = 64;
const PROCESS_DUP_HANDLE: u32 = 0x0040;
const DUPLICATE_SAME_ACCESS: u32 = 0x0000_0002;
/// 句柄表条目上限，防内核数据异常时死循环。
const MAX_HANDLES: usize = 1 << 22;

/// `<winternl.h>` / `<ntifs.h>` 里的 `SYSTEM_HANDLE_TABLE_ENTRY_INFO_EX`。
///
/// 字段用 `usize` 表示指针宽度，Rust 会按目标平台对齐：x64 是 40 字节，
/// x86 是 28 字节。手写固定偏移会在 32 位目标上错位。
#[repr(C)]
struct HandleEntryEx {
    object: usize,
    unique_process_id: usize,
    handle_value: usize,
    granted_access: u32,
    creator_back_trace_index: u16,
    object_type_index: u16,
    handle_attributes: u32,
    reserved: u32,
}

// 显式声明链接库，不依赖默认链接集：
//   - kernel32：GetStdHandle / SetStdHandle / GetFileType / OpenProcess /
//     DuplicateHandle / CloseHandle / CompareObjectHandles / QueryFullProcessImageNameW
//   - ntdll：NtQuerySystemInformation（该 API 没有 kernel32 的等价替代）
//
// 为什么不能省：`CompareObjectHandles` 是 Windows 10 才进的 kernel32，早期
// 导入库可能没有；`NtQuerySystemInformation` 从来不在 kernel32 里。若不写明，
// 本地 check 照样通过，到 CI 链接阶段才报 undefined symbol——那正是"看起来
// 编过了其实没链上"的典型。
#[link(name = "kernel32")]
extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn SetStdHandle(which: u32, h: Handle) -> i32;
    fn GetFileType(h: Handle) -> u32;
    fn GetCurrentProcess() -> Handle;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn DuplicateHandle(
        src_proc: Handle,
        src: Handle,
        dst_proc: Handle,
        dst: *mut Handle,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn CompareObjectHandles(a: Handle, b: Handle) -> i32;
    fn QueryFullProcessImageNameW(h: Handle, flags: u32, buf: *mut u16, size: *mut u32) -> i32;
}

#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(
        class: u32,
        info: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> NtStatus;
}

pub fn support() -> Support {
    Support {
        os: "windows",
        functional: true,
        detail: "枚举系统句柄表比对管道对象，SetStdHandle 改道（不做链式追踪）",
    }
}

pub fn stdout_pipe() -> Option<String> {
    let h = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if h.is_null() || h as isize == -1 {
        return None;
    }
    let t = unsafe { GetFileType(h) };
    (t == FILE_TYPE_PIPE).then(|| format!("{:x}", h as usize))
}

pub fn stderr_is_stdout() -> bool {
    let a = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let b = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    if a.is_null() || b.is_null() {
        return true;
    }
    unsafe { CompareObjectHandles(a, b) != 0 }
}

pub fn redirect_stdout_to_stderr() -> bool {
    let e = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    if e.is_null() || e as isize == -1 {
        return false;
    }
    // 必须在 stdout 第一次被写入之前调用：Rust 的 stdout 在首次使用时抓一次
    // 句柄，之后不再问内核。`guard::install()` 位于 main 第一行，满足这个前提。
    unsafe { SetStdHandle(STD_OUTPUT_HANDLE, e) != 0 }
}

fn image_name(proc: Handle) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut size = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(proc, 0, buf.as_mut_ptr(), &mut size) };
    if ok == 0 || size == 0 {
        return None;
    }
    String::from_utf16(&buf[..size as usize]).ok()
}

/// 取整张句柄表。返回原始字节与条目数。
fn handle_table() -> Option<(Vec<u8>, usize)> {
    // 起手 1 MiB，遇到 STATUS_INFO_LENGTH_MISMATCH 就按内核给的尺寸翻倍重试。
    let mut want: usize = 1 << 20;
    for _ in 0..8 {
        let mut buf = vec![0u8; want];
        let mut ret: u32 = 0;
        let st = unsafe {
            NtQuerySystemInformation(
                SYSTEM_EXTENDED_HANDLE_INFORMATION,
                buf.as_mut_ptr(),
                want as u32,
                &mut ret,
            )
        };
        if st == 0 {
            let n = usize::from_ne_bytes({
                let mut b = [0u8; std::mem::size_of::<usize>()];
                b.copy_from_slice(&buf[..std::mem::size_of::<usize>()]);
                b
            });
            return Some((buf, n.min(MAX_HANDLES)));
        }
        if st != STATUS_INFO_LENGTH_MISMATCH {
            return None;
        }
        want = (ret as usize).max(want * 2);
    }
    None
}

pub fn find_truncator(_pipe: &str, _me: &str) -> Option<String> {
    let mine = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if mine.is_null() {
        return None;
    }
    let (buf, n) = handle_table()?;
    let hdr = std::mem::size_of::<usize>() * 2;
    let ent = std::mem::size_of::<HandleEntryEx>();
    let cur = unsafe { GetCurrentProcess() };
    let me_pid = std::process::id();

    for i in 0..n {
        let off = hdr + i * ent;
        if off + ent > buf.len() {
            break;
        }
        // 按字段宽度读出条目，避免在未对齐的缓冲上做引用转换。
        let read_usize = |o: usize| -> usize {
            let mut b = [0u8; std::mem::size_of::<usize>()];
            b.copy_from_slice(&buf[o..o + std::mem::size_of::<usize>()]);
            usize::from_ne_bytes(b)
        };
        let pid = read_usize(off + std::mem::size_of::<usize>()) as u32;
        if pid == 0 || pid == me_pid {
            continue;
        }
        let hval = read_usize(off + std::mem::size_of::<usize>() * 2) as Handle;
        if hval.is_null() {
            continue;
        }

        let proc = unsafe { OpenProcess(PROCESS_DUP_HANDLE, 0, pid) };
        if proc.is_null() {
            continue;
        }
        let mut dup: Handle = std::ptr::null_mut();
        let ok = unsafe {
            DuplicateHandle(proc, hval, cur, &mut dup, 0, 0, DUPLICATE_SAME_ACCESS)
        };
        let same = ok != 0 && unsafe { CompareObjectHandles(dup, mine) != 0 };
        if !dup.is_null() {
            unsafe { CloseHandle(dup) };
        }
        if !same {
            unsafe { CloseHandle(proc) };
            continue;
        }
        // 找到了：拿它的映像名判断是不是截断程序。
        let named = image_name(proc);
        unsafe { CloseHandle(proc) };
        if let Some(name) = named {
            if let Some(t) = truncator_of(&[name]) {
                return Some(t);
            }
        }
    }
    None
}
