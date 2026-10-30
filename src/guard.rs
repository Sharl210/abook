//! 截断防护：让 `abook … | head` 这类截断**无法成功**。
//!
//! ## 为什么要有这个模块
//!
//! 规则写得再强硬也只是"要求模型别做"。而截断恰好是最容易悄悄发生、又
//! 看起来像成功的失败：退出码 0、输出少了一截、没有任何错误信号。模型据此
//! 下结论时，读到的是残卷，而它无法察觉自己读到的是残卷。
//!
//! 所以把防线从"要求不要做"降到"做了也不生效"。实测：`abook list | head -3`
//! 原本只给 3 行（完整 201 行），全部内容静默消失。装上本模块后，`head` 拿不到
//! 任何一行（它只读 stdout，而 stdout 已改接 stderr），完整输出改由 stderr 抵达
//! 调用方。链式写法 `abook list | grep x | head -3`（54 条只剩 3 条）同样被拦。
//!
//! ## 全部在同一个可执行文件里
//!
//! 没有守护进程、没有后台服务、没有外部依赖：`Cargo.toml` 的 dependencies 为空，
//! 本模块不 spawn 任何进程，只读操作系统已有的进程表。abook 仍然是一个可以随便
//! 拷贝、随便挪位置的单文件。
//!
//! ## 三个平台各写一份，行为一致
//!
//! 「全平台架构都要有对应的一个实现」——每种平台用自己的系统调用做同一件事：
//!
//! | 平台 | 识别管道 | 找持有者 | 改道 |
//! |---|---|---|---|
//! | Linux / Android | `/proc/self/fd/1` 指向 `pipe:[N]` | 扫 `/proc/*/fd` 找持有同一管道者 | `dup2(2,1)` |
//! | macOS | `fstat(1)` 的 `st_ino`（管道即其句柄） | `libproc` 枚举 pid 与 fd | `dup2(2,1)` |
//! | Windows | `GetFileType(stdout)==FILE_TYPE_PIPE` | `NtQuerySystemInformation` 枚举句柄 + `CompareObjectHandles` | `SetStdHandle` |
//!
//! 平台不支持时**不静默**：`support()` 会给出具体原因，`install()` 记录下来，
//! `ABOOK_GUARD_DEBUG=1` 时可看到"这台机器上防护到底生效没有"——不靠文档承诺，
//! 靠当场可观察。

use std::process;

// 平台实现的选择。
//
// `guard-force-*` feature 只用于在别的平台上做类型检查（见 Cargo.toml），
// 编译期的真值仍是 `target_os`。把这两个 feature 与真值写成互斥，是为了
// 避免"强制"时出现两个 `imp` 同时定义。
#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    not(feature = "guard-force-macos"),
    not(feature = "guard-force-windows")
))]
#[path = "guard/procfs.rs"]
mod imp;

#[cfg(all(
    any(target_os = "macos", feature = "guard-force-macos"),
    not(feature = "guard-force-windows")
))]
#[path = "guard/macos.rs"]
mod imp;

#[cfg(all(
    any(target_os = "windows", feature = "guard-force-windows"),
    not(feature = "guard-force-macos")
))]
#[path = "guard/windows.rs"]
mod imp;

#[cfg(all(
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "windows"
    )),
    not(feature = "guard-force-macos"),
    not(feature = "guard-force-windows")
))]
#[path = "guard/none.rs"]
mod imp;

/// 会削减读入内容、且可能被挂在 abook 输出后面的程序。
///
/// 判据是"读完就丢一部分"：`head`/`tail`/`sed`/`awk`/`cut`/`dd` 都在其列。
/// `grep`/`sort`/`uniq`/`wc` 不在此列——它们过滤或统计，不截断。
///
/// 这份清单三平台共用：程序名是同一个概念，只有"怎么找到它"随平台不同。
const TRUNCATORS: &[&str] = &[
    "head", "tail", "sed", "awk", "gawk", "mawk", "nawk", "cut", "dd", "truncate", "more", "less",
    "most", "pg", "bat", "batcat", "sponge",
];

/// busybox / toybox 这类多合一程序：真正的截断器是第二个名字。
const MULTI_CALL: &[&str] = &["busybox", "toybox", "coreutils"];

/// Windows 上等价的截断程序。PowerShell 的 `Select-Object -First` 落在
/// powershell.exe 自己身上，只能按整程序名匹配，看不到参数。
const TRUNCATORS_WIN: &[&str] = &["more", "findstr", "head"];

fn basename_of(s: &str) -> &str {
    // 同时按 `/` 与 `\` 切：Windows 的可执行名用反斜杠。
    let a = s.rsplit('/').next().unwrap_or(s);
    a.rsplit('\\').next().unwrap_or(a)
}

fn strip_exe(s: &str) -> &str {
    for suf in [".exe", ".EXE", ".com", ".COM"] {
        if let Some(t) = s.strip_suffix(suf) {
            return t;
        }
    }
    s
}

fn in_list(name: &str, list: &[&str]) -> bool {
    list.iter().any(|t| t.eq_ignore_ascii_case(name))
}

/// 从一条 argv 判断它是不是截断程序；是就返回用于报告的程序名。
fn truncator_of(argv: &[String]) -> Option<String> {
    let first = argv.first()?;
    let stem = strip_exe(basename_of(first));
    if in_list(stem, TRUNCATORS) || in_list(stem, TRUNCATORS_WIN) {
        return Some(stem.to_string());
    }
    if in_list(stem, MULTI_CALL) {
        if let Some(sub) = argv.get(1) {
            let sstem = strip_exe(basename_of(sub));
            if in_list(sstem, TRUNCATORS) {
                return Some(format!("{stem} {sstem}"));
            }
        }
    }
    None
}

/// 平台能力报告。`detail` 说明"为什么不可用"，是给人看的一句话。
pub struct Support {
    pub os: &'static str,
    /// true = 本平台完整实现；false = 本平台不做，原因在 detail。
    pub functional: bool,
    pub detail: &'static str,
}

pub fn support() -> Support {
    imp::support()
}

fn debug_on() -> bool {
    std::env::var_os("ABOOK_GUARD_DEBUG").is_some()
}

/// 在程序最开头调用。**任何**失败都静默放过——防护绝不能妨碍正常输出。
pub fn install() {
    // 逃生门：自检与测试需要看到原始行为。
    if std::env::var_os("ABOOK_NO_GUARD").is_some() {
        if debug_on() {
            eprintln!("[abook guard] ABOOK_NO_GUARD 已设置，跳过");
        }
        return;
    }

    let sup = support();
    if !sup.functional {
        if debug_on() {
            eprintln!("[abook guard] 本平台不启用：{}（{}）", sup.os, sup.detail);
        }
        return;
    }

    // 只有 stdout 是管道时才可能被截断；写文件、写 socket 都不必管。
    let Some(pipe) = imp::stdout_pipe() else {
        if debug_on() {
            eprintln!("[abook guard] stdout 不是管道，无需防护（{}）", sup.os);
        }
        return;
    };

    // 沿管道链找截断程序。链式写法 `abook | grep x | head -3` 里我们管道的
    // 读端在 grep 手里，只看第一跳会漏——那正是最隐蔽的一种。
    let me = process::id().to_string();
    let Some(name) = imp::find_truncator(&pipe, &me) else {
        if debug_on() {
            eprintln!("[abook guard] 管道链里没有截断程序（{pipe}）");
        }
        return;
    };

    if imp::stderr_is_stdout() {
        eprintln!(
            "[abook] 检测到输出被 `{name}` 截断，但 stderr 与 stdout 指向同一管道，无法改道。\n\
             [abook] 本次命令的完整结果已被丢弃——请改用 --limit / --page / --per-page 控制篇幅，勿用截断管道。"
        );
        return;
    }

    if !imp::redirect_stdout_to_stderr() {
        eprintln!(
            "[abook] 检测到输出被 `{name}` 截断，但改道失败，完整结果可能已被丢弃。\n\
             [abook] 请勿对 abook 输出使用截断管道；要控制篇幅用 --limit / --page / --per-page。"
        );
        return;
    }

    eprintln!(
        "[abook] 截断防护已生效：检测到输出被 `{name}` 截断，完整结果已改由 stderr 输出（未损失任何内容）。\n\
         [abook] 以后请勿对 abook 的输出使用截断管道；要控制篇幅用 --limit / --page / --per-page。"
    );
}
