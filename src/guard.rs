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

/// 无条件视为截断的程序：它们的存在意义就是"只取一部分"。
///
/// `head -3` / `tail -3` / `head -n 5` 都是明确的截断，不需看参数。
const ALWAYS: &[&str] = &["head", "tail"];

/// 看参数才知道是不是截断的程序。这类既能截断也能做纯信息处理，
/// 一刀切会误伤——见 `truncator_of` 里的说明。
const SOMETIMES: &[&str] = &["sed", "awk", "gawk", "mawk", "nawk", "cut", "dd"];

/// Windows 上的等价物。PowerShell 的 `Select-Object -First` 落在
/// powershell.exe 自己身上，只能按整程序名匹配，看不到参数。
const ALWAYS_WIN: &[&str] = &["more", "findstr"];

/// busybox / toybox 这类多合一程序：真正的截断器是第二个名字。
const MULTI_CALL: &[&str] = &["busybox", "toybox", "coreutils"];

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

/// `sed` 是否在截断。
///
/// 判据是**行号范围**，不是"用了 sed"：
///
/// | 写法 | 行为 | 判定 |
/// |---|---|---|
/// | `sed -n '1,3p'` | 只输出前 3 行 | 截断 |
/// | `sed -n '1p'` / `'$p'` | 只输出 1 行 | 截断 |
/// | `sed -n 's/.*src=\(.*\)/\1/p'` | 提取字段，行数不变 | 放行 |
/// | `sed -n '/foo/p'` | 正则过滤，类似 grep | 放行 |
/// | `sed 's/a/b/'` | 无 `-n`，全部行输出 | 放行 |
/// | `sed -n '1,3p;5q'` | 带 `q` 提前退出 | 截断 |
fn sed_truncates(argv: &[String]) -> bool {
    // 必须有 `-n`：没有它 sed 默认打印每一行，不会减少内容。
    let mut quiet = false;
    let mut script = String::new();
    let mut i = 1;
    while i < argv.len() {
        let a = &argv[i];
        if a == "-n" || a == "--quiet" || a == "--silent" {
            quiet = true;
        } else if a == "-e" || a == "--expression" {
            if let Some(v) = argv.get(i + 1) {
                script.push_str(v);
                script.push(';');
            }
            i += 1;
        } else if a.starts_with("--") || (a.starts_with('-') && a.len() > 1) {
            // 其他选项（-i / -E / -r / -f …）不影响此判定
        } else if script.is_empty() {
            script.push_str(a);
        }
        i += 1;
    }
    if !quiet {
        return false;
    }
    // 含 `s` 替换命令的，输出的是改写后的整行，不是残卷。
    let has_substitute = script.contains('s');
    // 提前退出等同截断。
    if script.split(';').any(|c| c.trim().ends_with('q')) {
        return true;
    }
    // 行号范围打印：`1,3p` / `1p` / `$p` / `1,$p`
    let mut num = String::new();
    for part in script.split(|c| c == ';' || c == '\n') {
        let t = part.trim();
        num.clear();
        let mut has_range = false;
        for ch in t.chars() {
            match ch {
                '0'..='9' | '$' => {
                    num.push(ch);
                    has_range = true;
                }
                ',' => {
                    if !num.is_empty() {
                        num.push(',');
                    }
                }
                'p' => {
                    if has_range && !num.is_empty() && !has_substitute {
                        return true;
                    }
                    break;
                }
                _ => break, // 遇到 `s`、`/` 等说明不是纯行号
            }
        }
    }
    false
}

/// `awk` 是否在截断：有 `exit` 或 `NR` 比较就是。`{print $1}` 只是选列，放行。
fn awk_truncates(argv: &[String]) -> bool {
    let mut prog = String::new();
    let mut i = 1;
    while i < argv.len() {
        let a = &argv[i];
        if a == "-F" || a == "-v" || a == "-f" {
            i += 1; // 跳过该选项的值
        } else if !a.starts_with('-') && prog.is_empty() {
            prog.push_str(a);
        }
        i += 1;
    }
    if prog.is_empty() {
        return false;
    }
    let p = prog.replace(' ', "");
    if p.contains("exit") {
        return true;
    }
    // NR<=10 / NR==5 / NR<3 都意味着"到点就停"
    let has_nr = p.contains("NR");
    let cmpHit = p.contains("<") || p.contains("==");
    has_nr && cmpHit
}

/// `cut` 是否在截断：`-c` 按字符切会丢内容；`-f` 只选列，行数不变。
fn cut_truncates(argv: &[String]) -> bool {
    for a in argv.iter().skip(1) {
        if a == "-c" || a == "--characters" || a.starts_with("-c") && !a.starts_with("-d") {
            return true;
        }
        if a.starts_with("--characters=") {
            return true;
        }
    }
    false
}

/// `dd` 是否在截断：只有带 `count=` 才会提前停。
fn dd_truncates(argv: &[String]) -> bool {
    argv.iter().skip(1).any(|a| a.starts_with("count="))
}

/// 从一条 argv 判断它是不是**真的在截断**；是就返回用于报告的程序名。
///
/// ## 为什么必须看参数，而不是只看程序名
///
/// 第一版把 `sed`/`awk`/`cut` 一并当作截断器，结果 CI 的发布校验脚本被拦：
///
/// ```sh
/// EXPECT=$(./target/release/abook version | sed -n 's/.*src=\([0-9a-f]*\).*/\1/p')
/// ```
///
/// 这是**提取字段**——输出短，但信息完整（就是那个指纹）。防护把输出改道到
/// stderr，`$(...)` 只捕获 stdout，于是 `EXPECT` 为空，整个 15 目标发布会话
/// 卡死在这一步。
///
/// 教训：**误报的代价比漏报更大**。漏报只是没拦住违规，模型读到残卷；误报会
/// 打断本来正确的流程，而且现场表现为"工具坏了"，很难第一时间联想到防护。
/// 所以判据从严：只有确证会削减内容的写法才拦。
fn truncator_of(raw: &[String]) -> Option<String> {
    // Android（bionic）的 /proc/<pid>/cmdline 会多出一项：第一项是进程名，
    // 第二项又是完整的 argv[0]。实测 `sed -n '1,3p'` 读出来是：
    //
    //     ["sed", "/data/.../bin/sed", "-n", "1,3p"]
    //
    // 不处理这一项，脚本参数就从 `/data/.../bin/sed` 开始解析——那个字符串
    // 里含 `s`，被判成"含替换命令"，于是 `sed -n '1,3p'` 这个真截断被放行，
    // 而判定结果与真实行为完全相反。这类错误比不实现更糟：它给出确定的错答案。
    let owned;
    let argv: &[String] = match (raw.first(), raw.get(1)) {
        (Some(a), Some(b)) if b.contains('/') && basename_of(b) == basename_of(a) => {
            owned = std::iter::once(a.clone())
                .chain(raw.iter().skip(2).cloned())
                .collect::<Vec<String>>();
            &owned
        }
        _ => raw,
    };

    let first = argv.first()?;
    let stem = strip_exe(basename_of(first));
    if in_list(stem, ALWAYS) || in_list(stem, ALWAYS_WIN) {
        return Some(stem.to_string());
    }
    if in_list(stem, SOMETIMES) {
        let hit = match stem {
            "sed" => sed_truncates(argv),
            "awk" | "gawk" | "mawk" | "nawk" => awk_truncates(argv),
            "cut" => cut_truncates(argv),
            "dd" => dd_truncates(argv),
            _ => false,
        };
        if hit {
            return Some(stem.to_string());
        }
    }
    if in_list(stem, MULTI_CALL) {
        if let Some(sub) = argv.get(1) {
            let sstem = strip_exe(basename_of(sub));
            if in_list(sstem, ALWAYS) {
                return Some(format!("{stem} {sstem}"));
            }
            if in_list(sstem, SOMETIMES) {
                // 多合一程序后面的参数整体右移一位
                let mut rest = vec![sstem.to_string()];
                rest.extend(argv.iter().skip(2).cloned());
                let hit = match sstem {
                    "sed" => sed_truncates(&rest),
                    "awk" | "gawk" | "mawk" | "nawk" => awk_truncates(&rest),
                    "cut" => cut_truncates(&rest),
                    "dd" => dd_truncates(&rest),
                    _ => false,
                };
                if hit {
                    return Some(format!("{stem} {sstem}"));
                }
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
