//! Windows 实现：动态加载，不依赖任何非默认导入库。
//!
//! ## 为什么全部走 GetProcAddress
//!
//! 第一版把 API 写成 `extern "system"` 声明 + `#[link(name = "ntdll")]`，
//! 结果 CI 上**三个 Windows 目标全部编译失败**，而本地 `cargo check` 通过——
//! 因为 `check` 不做链接。
//!
//! 根因是 gnullvm 目标的默认链接库只有
//! `-lmingw32 -lmingwex -lmsvcrt -lkernel32 -luser32`（见 rustc 的
//! `windows_gnullvm` target spec），**没有 ntdll**。`NtQuerySystemInformation`
//! 只在 ntdll 里，显式 `#[link]` 时链接器找不到导入库就整体失败。
//!
//! 改成运行时取地址之后，编译期只需要 kernel32 的 `LoadLibraryW` /
//! `GetModuleHandleW` / `GetProcAddress`——这三个都在默认库列表里。同时还有
//! 两个附带好处：
//!
//!   1. `CompareObjectHandles` 是 Windows 10 才进 kernel32 的，动态取不到时
//!      可以优雅降级，而不是让整个二进制在旧系统上加载失败。
//!   2. 不再有任何 `#[link]`，跨目标编译不会再出现"本地过、CI 挂"的分裂。
//!
//! ## 已知限制（诚实记录）
//!
//! **不做链式追踪。** 句柄表只说"某进程持有某对象"，不说"这是它的 stdout"，
//! 所以拿不到下一跳。Linux 那边能追 `abook | grep x | head -3`，Windows 只能
//! 拦住直接写在管道后面的那一个截断程序。`abook | more` 这类常见写法不受影响。

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

// 这三个是 kernel32 的，rustc 在 gnullvm 上默认就链 kernel32。
// 只留它们，不写任何 #[link]。
extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn SetStdHandle(which: u32, h: Handle) -> i32;
    fn GetFileType(h: Handle) -> u32;
    fn GetCurrentProcess() -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
}

/// 把 `&str` 转成以 NUL 结尾的宽字符串，用于 LoadLibraryW / GetModuleHandleW。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 取一个导出函数的地址。函数名是 ASCII，按字节查。
fn sym(module: &str, name: &str) -> Option<*mut c_void> {
    let w = wide(module);
    // 优先已加载的模块：kernel32/ntdll 在任何进程里都已经在了。
    let m = unsafe { GetModuleHandleW(w.as_ptr()) };
    let m = if m.is_null() {
        unsafe { LoadLibraryW(w.as_ptr()) }
    } else {
        m
    };
    if m.is_null() {
        return None;
    }
    let mut n: Vec<u8> = name.bytes().collect();
    n.push(0);
    let p = unsafe { GetProcAddress(m, n.as_ptr()) };
    (!p.is_null()).then_some(p)
}

macro_rules! api {
    ($ty:ty, $module:expr, $name:expr) => {{
        static ONCE: std::sync::OnceLock<Option<$ty>> = std::sync::OnceLock::new();
        *ONCE.get_or_init(|| sym($module, $name).map(|p| unsafe { std::mem::transmute(p) }))
    }};
}

/// `<ntifs.h>` 的 `SYSTEM_HANDLE_TABLE_ENTRY_INFO_EX`。
///
/// 用 `usize` 表示指针宽度，让 Rust 按目标平台对齐：x64 是 40 字节，x86 是
/// 28 字节。手写固定偏移会在 32 位目标上错位。目前只用前三个字段，其余字段
/// 只为把条目尺寸算对（步进靠 `size_of`）。
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

pub fn support() -> Support {
    // 缺关键 API 时如实报告不可用，而不是让防护静默不生效。
    let ok = sym("kernel32", "GetFileType").is_some()
        && sym("ntdll", "NtQuerySystemInformation").is_some()
        && sym("kernel32", "CompareObjectHandles").is_some();
    Support {
        os: "windows",
        functional: ok,
        detail: if ok {
            "枚举系统句柄表比对管道对象，SetStdHandle 改道（不做链式追踪）"
        } else {
            "缺少 NtQuerySystemInformation 或 CompareObjectHandles（需 Windows 10 及以上）"
        },
    }
}

pub fn stdout_pipe() -> Option<String> {
    let h = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if h.is_null() || h as isize == -1 {
        return None;
    }
    (unsafe { GetFileType(h) } == FILE_TYPE_PIPE).then(|| format!("{:x}", h as usize))
}

pub fn stderr_is_stdout() -> bool {
    let a = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let b = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    if a.is_null() || b.is_null() {
        return true;
    }
    match api!(
        unsafe extern "system" fn(Handle, Handle) -> i32,
        "kernel32",
        "CompareObjectHandles"
    ) {
        Some(f) => unsafe { f(a, b) != 0 },
        // 拿不到比较函数时保守当同一路：宁可只警告，也别在不确定时改道。
        None => true,
    }
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
    type F = unsafe extern "system" fn(Handle, u32, *mut u16, *mut u32) -> i32;
    let f = api!(F, "kernel32", "QueryFullProcessImageNameW")?;
    let mut buf = vec![0u16; 1024];
    let mut size = buf.len() as u32;
    if unsafe { f(proc, 0, buf.as_mut_ptr(), &mut size) } == 0 || size == 0 {
        return None;
    }
    String::from_utf16(&buf[..size as usize]).ok()
}

/// 取整张句柄表：返回缓冲与条目数。
fn handle_table() -> Option<(Vec<u8>, usize)> {
    type F = unsafe extern "system" fn(u32, *mut u8, u32, *mut u32) -> NtStatus;
    let f = api!(F, "ntdll", "NtQuerySystemInformation")?;
    // 起手 1 MiB，遇到 STATUS_INFO_LENGTH_MISMATCH 就按内核给的尺寸放大重试。
    let mut want: usize = 1 << 20;
    for _ in 0..8 {
        let mut buf = vec![0u8; want];
        let mut ret: u32 = 0;
        let st = unsafe {
            f(
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
    type OpenF = unsafe extern "system" fn(u32, i32, u32) -> Handle;
    type DupF = unsafe extern "system" fn(Handle, Handle, Handle, *mut Handle, u32, i32, u32) -> i32;
    type CmpF = unsafe extern "system" fn(Handle, Handle) -> i32;

    let open = api!(OpenF, "kernel32", "OpenProcess")?;
    let dup = api!(DupF, "kernel32", "DuplicateHandle")?;
    let cmp = api!(CmpF, "kernel32", "CompareObjectHandles")?;

    let mine = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if mine.is_null() {
        return None;
    }
    let (buf, n) = handle_table()?;
    let hdr = std::mem::size_of::<usize>() * 2;
    let ent = std::mem::size_of::<HandleEntryEx>();
    let cur = unsafe { GetCurrentProcess() };
    let me_pid = std::process::id();
    let pw = std::mem::size_of::<usize>();

    let read_usize = |o: usize| -> usize {
        let mut b = [0u8; std::mem::size_of::<usize>()];
        b.copy_from_slice(&buf[o..o + std::mem::size_of::<usize>()]);
        usize::from_ne_bytes(b)
    };

    for i in 0..n {
        let off = hdr + i * ent;
        if off + ent > buf.len() {
            break;
        }
        let pid = read_usize(off + pw) as u32;
        if pid == 0 || pid == me_pid {
            continue;
        }
        let hval = read_usize(off + pw * 2) as Handle;
        if hval.is_null() {
            continue;
        }

        let proc = unsafe { open(PROCESS_DUP_HANDLE, 0, pid) };
        if proc.is_null() {
            continue;
        }
        let mut duph: Handle = std::ptr::null_mut();
        let ok = unsafe { dup(proc, hval, cur, &mut duph, 0, 0, DUPLICATE_SAME_ACCESS) };
        let same = ok != 0 && unsafe { cmp(duph, mine) != 0 };
        if !duph.is_null() {
            unsafe { CloseHandle(duph) };
        }
        if !same {
            unsafe { CloseHandle(proc) };
            continue;
        }
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
