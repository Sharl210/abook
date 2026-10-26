//! `abook upgrade` — 把"下载新版本并装好"这件事收进工具自己。
//!
//! 此前这套流程写在给 AI 读的规范里:判断系统与架构、在 PATH 里挑用户级目录、
//! 按顺序试五个下载源、验五条、原子替换。写在文档里的流程有两个毛病——占篇幅,
//! 而且每一轮都要人重走一遍,任何一步记错就变成"装了个坏二进制还报成功"。
//!
//! 设计上的取舍:
//!
//! 1. **源按序回退,但都是同构地址**。GitHub 直连一个,后面三个是它的镜像代理。
//!    镜像只是把同一个原始 URL 套进各自的前缀,所以取文件与取 manifest 走同一套
//!    拼装逻辑,不需要为每个源写特例。jsDelivr 是唯一的形态差异(它用
//!    `@branch` 表示分支),但拼完之后仍然是"base + 相对路径",一样吃这套。
//!
//! 2. **先看 manifest 再决定要不要下载**。manifest 带 sha256 与版本号,能在下载
//!    1.6MB 之前就回答"已经是最新了"和"下来的字节是不是完整"。没有它就只能
//!    下完再说,而"下完再说"意味着每次检查都付一次带宽。
//!
//! 3. **替换的是当前正在运行的那个文件**,不是硬编码的 `~/.local/bin/abook`。
//!    二进制可能被装在别处(PATH 上的任何一个目录、或是包管理器管的路径),猜
//!    安装位置就会在升级时写出一份影子副本,让用户机器上出现两个版本、哪个生
//!    效取决于 PATH 顺序。
//!
//! 4. **先让它跑起来自检,再认为装好了**。一个能装但跑不起来的 release 是坏
//!    release,只在警告行里说一句,脚本和 CI 会把它读成成功。

use crate::sha256 as sh;
use crate::util::*;
use crate::Args;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 下载源。顺序即回退顺序:能直连就直连,直连不通才付代理的延迟。
///
/// 三个镜像都是 raw.githubusercontent.com 的前缀代理,所以 `{base}/{rel}` 这套
/// 拼装对它们全部成立。jsDelivr 用 `gh/{repo}@{branch}` 表示分支,形态不同但拼
/// 完同样是"base + 相对路径",因此并列在这里不破坏统一处理。
/// 这次要试哪些源。
///
/// 关键规则：调用方显式给了源，就**只**用它，不回退。
///
/// 把 `--url` 插到列表最前面然后继续回退，会造成一个很难察觉的假验证：用户以为
/// 在内网镜像上验证新版，实际镜像不可达后被 GitHub 接上，最后打印"已是最新"。
/// 屏幕上只有一行"不可达"紧跟一个成功结论，没有任何提示说"你指定的源没被用上"。
/// 验证的意义就是确认那个源能用，悄悄换源等于把验证做废。
fn pick_sources(repo: &str, url: Option<&str>) -> Vec<(String, String)> {
    match url {
        Some(u) => vec![("--url".to_string(), u.trim_end_matches('/').to_string())],
        None => sources(repo),
    }
}

fn sources(repo: &str) -> Vec<(String, String)> {
    let raw = format!("https://raw.githubusercontent.com/{repo}/downloads");
    vec![
        ("GitHub".to_string(), raw.clone()),
        ("gh-proxy".to_string(), format!("https://gh-proxy.com/{raw}")),
        ("ghfast".to_string(), format!("https://ghfast.top/{raw}")),
        ("ghproxy.net".to_string(), format!("https://ghproxy.net/{raw}")),
        (
            "jsDelivr".to_string(),
            format!("https://cdn.jsdelivr.net/gh/{repo}@downloads"),
        ),
    ]
}

/// 把 `1.6.15` 这类版本切成可比较的数字段。
///
/// 直接按字符串比大小会让 `1.6.9` 排到 `1.6.15` 后面——这正是"检查更新"最典型
/// 的错法,而且它的表现是"永远说已是最新",没有任何报错可循。预发布后缀
/// (`-rc1`、`+build`)不参与比较,只比前三段数字。
fn ver_tuple(v: &str) -> (u64, u64, u64) {
    let core = v.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next().unwrap_or(core);
    let mut it = core.split('.').map(|s| {
        s.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u64>()
            .unwrap_or(0)
    });
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// 从 manifest 里抠出本平台那条记录。返回 (sha256, size, version)。
///
/// 手写解析而不是引一个 JSON 依赖:整个工具零外部依赖是既有约定,而这里只需要
/// 一个对象里的一层数组。找的是 `"target":"<三元组>"` 那一段,然后在它之后读到
/// 下一个 `}` 为止——字段顺序由 CI 固定生成,不会跨行跳动。
fn pick_entry(manifest: &str, target: &str) -> Option<(String, u64, String)> {
    // 按 JSON 的语义找条目，不做"紧凑字符串"假设。
    //
    // 原先用 `"target":"<triple>"` 这种把冒号两侧空白都写死的模式，而 CI 恰好
    // 生成的就是紧凑格式，于是它一直工作。一旦用 `python3 -m json.tool`、
    // `jq` 或 `json.dumps` 重新格式化（冒号后多一个空格），查找立刻落空——
    // 而报出来的话是"里面没有 x86_64-unknown-linux-musl"，把 JSON 解析问题
    // 说成了"这个源没有你要的架构"。
    //
    // 同一份 manifest 换个工具打印就不认，说明解析写错了对象：manifest 是 JSON，
    // 不是某个脚本拼接出来的固定格式文本。
    let key = format!("\"target\"");
    let mut from = 0usize;
    let at = loop {
        let at = manifest[from..].find(&key)? + from;
        // 取 `"target"` 之后、下一个 `"` 之前的值，逐字符跳过空白。
        let rest = &manifest[at + key.len()..];
        let rest = rest.trim_start();
        let rest = rest.strip_prefix(':')?;
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('"')?;
        let end = rest.find('"')?;
        if &rest[..end] == target {
            break at;
        }
        from = at + key.len();
    };
    let tail = &manifest[at..];
    let end = tail.find('}')?;
    let obj = &tail[..end];
    // 字符串字段。`size` 不走这里——它在 manifest 里是裸数字(`"size":1617896`),
    // 加引号去找会静默读成 0,而 0 在"跳过下载"和"校验"两处都被当成合法值。
    // 字段提取同样不能把冒号两侧的空白写死。`"sha256":"..."` 只在紧凑排版下
    // 成立，而 `"sha256": "..."` 是同一个 JSON 的另一种合法写法——换了排版就
    // 读成 None，再往上表现成"这个源没有你要的架构"。
    let sfield = |name: &str| -> Option<String> {
        let k = format!("\"{}\"", name);
        let at = obj.find(&k)? + k.len();
        let rest = obj[at..].trim_start().strip_prefix(':')?.trim_start();
        let rest = rest.strip_prefix('"')?;
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    };
    let nfield = |name: &str| -> Option<u64> {
        let k = format!("\"{}\":", name);
        let s = obj.find(&k)? + k.len();
        obj[s..]
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u64>()
            .ok()
    };
    let sha = sfield("sha256")?;
    let size = nfield("size").unwrap_or(0);
    // 0 字节不可能是一份能跑的二进制;manifest 坏的越早停越好。
    if sha.is_empty() || size == 0 {
        return None;
    }
    // version 在文件顶层而不是条目里;老 manifest 没有它,回落到空。
    // 同样不假设紧凑排版:第三处把空白写死的地方,改一处漏另一处的效果是
    // "条目认出来了、版本读不到",而提示语会说成"manifest 未声明版本"。
    let version = {
        let k = "\"version\"";
        manifest
            .find(k)
            .and_then(|at| {
                let rest = manifest[at + k.len()..].trim_start();
                let rest = rest.strip_prefix(':')?.trim_start();
                let rest = rest.strip_prefix('"')?;
                let end = rest.find('"')?;
                Some(rest[..end].to_string())
            })
            .unwrap_or_default()
    };
    Some((sha, size, version))
}

fn curl() -> bool {
    Command::new("curl")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 下载到指定路径。返回成功与否,失败原因交给调用方按源逐条汇总。
///
/// 没有 `://` 的地址当作本地目录:内网共享盘、离线机上的 U 盘镜像、以及自己做
/// 镜像验证时都会用到一个裸路径,而 curl 对裸路径只会报"协议不支持"。
fn fetch(url: &str, to: &Path) -> bool {
    if !url.contains("://") {
        return std::fs::copy(url, to).is_ok();
    }
    let mut c = Command::new("curl");
    // `--connect-timeout` 单独给:被墙的表现是连接阶段挂住而不是收到拒绝,只靠
    // 总超时会让每个源白等满 90 秒×5 个源。
    c.args([
        "-fsSL",
        "--connect-timeout",
        "6",
        "--max-time",
        "90",
        "--retry",
        "2",
        "--retry-delay",
        "1",
        "-o",
    ])
    .arg(to)
    .arg(url);
    c.status().map(|s| s.success()).unwrap_or(false)
}

/// 文件头像不像本平台的可执行文件。
///
/// 镜像站被劫持、或代理返回一页 HTML 时,HTTP 状态是 200、内容却不是二进制。
/// 只信状态码会把一页"页面不存在"装成 abook。
fn looks_exec(b: &[u8]) -> bool {
    if b.len() < 64 {
        return false;
    }
    let elf = b.starts_with(b"\x7fELF");
    let macho = b.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
        || b.starts_with(&[0xce, 0xfa, 0xed, 0xfe])
        || b.starts_with(&[0xca, 0xfe, 0xba, 0xbe]);
    // Windows PE: `MZ` 开头,真正的 PE 头在 e_lfanew 处。
    let pe = b.starts_with(b"MZ");
    elf || macho || pe
}

fn read_version_of(p: &Path) -> Option<String> {
    let o = Command::new(p).arg("version").output().ok()?;
    if !o.status.success() {
        return None;
    }
    let t = String::from_utf8_lossy(&o.stdout);
    // 形如 `abook 1.6.15 (linux/x86_64) src=...`
    t.split_whitespace().nth(1).map(|s| s.to_string())
}

/// 进度信息走哪一路。
///
/// `--json` 模式下 stdout 只能有那一个 JSON 对象，任何进度行混进去都会让调用方
/// 的解析器直接失败——而失败点在对方，不在我们这边，很难归因。所以这里把进度
/// 统一改道 stderr：人在终端照样看得见（stderr 不缓冲、直接上屏），机器读
/// stdout 只拿到 JSON。
macro_rules! prog {
    ($json:expr, $($t:tt)*) => {
        if $json { eprintln!($($t)*) } else { println!($($t)*) }
    };
}

/// 把一次检查的结论发出去。
///
/// 抽成函数是因为"检查结论"有三条出口（有新版本、已是最新、本地更新），它们
/// 分散在版本比较和 `--check` 两处。原先只有一处写了 JSON，另外两处在 `--json`
/// 下照样打印人话——于是同一个命令的输出形态取决于远端状态，调用方没法预先
/// 知道该按哪种解析。
#[allow(clippy::too_many_arguments)]
fn emit_check_result(
    json: bool,
    os: &str,
    arch: &str,
    target: &str,
    remote_ver: &str,
    size: u64,
    state: &str,
    from: &str,
    install_to: &std::path::Path,
) {
    if !json {
        return;
    }
    println!(
        "{{\"os\":{},\"arch\":{},\"target\":{},\"current\":{},\"remote\":{},\"size\":{},\"state\":{},\"source\":{},\"install_to\":{}}}",
        jstr(os),
        jstr(arch),
        jstr(target),
        jstr(crate::VERSION),
        jstr(remote_ver),
        size,
        jstr(state),
        jstr(from),
        jstr(&install_to.to_string_lossy())
    );
}

pub fn cmd_upgrade(a: &Args) -> R<i32> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let target = match crate::target_triple(os, arch) {
        Some(t) => t,
        None => return usage(format!("不支持的平台: {os}/{arch}")),
    };
    let exe = if os == "windows" { "abook.exe" } else { "abook" };
    let repo = std::env::var("ABOOK_REPO").unwrap_or_else(|_| "Sharl210/abook".to_string());

    let check_only = a.has("check");
    let json = a.has("json");
    let force = a.has("force");

    // 更新目标是"正在跑的这个文件"。`current_exe` 在 Linux 上读 /proc/self/exe,
    // 已经解析过符号链接,所以它指向真正被执行的那一份。
    let cur = std::env::current_exe()
        .map_err(|e| EE::Io(format!("拿不到当前程序路径: {e}")))?;
    let cur = std::fs::canonicalize(&cur).unwrap_or(cur);
    let install_to: PathBuf = match a.get("dir") {
        Some(d) => Path::new(d).join(exe),
        None => cur.clone(),
    };

    // `--dry-run` 是"只看将要做什么"，可以先于联网返回；`--json` 不是。
    //
    // 原先两者合在一个条件里，于是 `abook upgrade --check --json` 从不取
    // manifest：输出里有 `version` 字段、退出码 0，脚本会读成"检查过了、已是
    // 当前版本"——它检查的是自己的命令行，不是远端。机器可读输出恰恰是"静默
    // 假成功"代价最大的地方，因为没有人会去看它的措辞。
    //
    // 这里的源列表也必须走 `pick_sources`。用 `sources()` 会把显式 `--url`
    // 顶替掉，而这条命令的用途之一正是"报告我打算用哪个源"——报告错了比不报
    // 更糟。
    if a.has("dry-run") {
        let srcs = pick_sources(&repo, a.get("url"));
        println!(
            "{{\"os\":{},\"arch\":{},\"target\":{},\"current\":{},\"install_to\":{},\"version\":{},\"sources\":[{}]}}",
            jstr(os),
            jstr(arch),
            jstr(target),
            jstr(&cur.to_string_lossy()),
            jstr(&install_to.to_string_lossy()),
            jstr(crate::VERSION),
            srcs.iter()
                .map(|(n, u)| format!("{{\"name\":{},\"base\":{}}}", jstr(n), jstr(u)))
                .collect::<Vec<_>>()
                .join(",")
        );
        return Ok(0);
    }

    if !curl() {
        return blocked(
            "本机没有 curl,无法下载。请手动取一份对应架构的产物放进 PATH。".to_string(),
        );
    }

    let dir = install_to
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(home_dir);

    prog!(json, "当前 {} ({os}/{arch})", crate::VERSION);
    prog!(json, "目标 {target}");
    prog!(json, "安装位置 {}", install_to.display());
    prog!(json, "");

    // 临时文件放在目标同目录:跨文件系统 rename 会退化成"复制+删除",那样中途
    // 中断就会留下半个可执行文件。`.part` 前缀用点开头,不会被执行。
    let part = dir.join(format!(".{exe}.upgrade"));
    let _ = std::fs::remove_file(&part);

    // `--url` 必须同时作用于 manifest 与文件。只作用一半会出现"文件从镜像取、
    // manifest 从 GitHub 取",而镜像存在的理由恰恰是 GitHub 不通——那时它两个都
    // 取不到,却报得像只有文件失败。
    // 用户显式给了源，就只用它。
    //
    // 原先是把 `--url` 插到列表最前面然后继续回退，于是 `--url` 指向一个不可达的
    // 镜像时，后面的 GitHub 会接上并成功，最终打印"已是最新"。用户以为自己在测
    // 内网镜像，实际测的永远是公网——而回退发生时屏幕上也只是一行"不可达"，后面
    // 紧跟一个成功结论，没有任何提示说"你指定的源没被用上"。
    //
    // 指定了源却悄悄换了源，等于把"验证"这件事做废：验证的意义就是确认那个源能用。
    let srcs = pick_sources(&repo, a.get("url"));
    let explicit = a.get("url").is_some();
    let mut failures: Vec<String> = Vec::new();
    let mut entry: Option<(String, u64, String, String)> = None; // sha,size,ver,源名
    let mpath = dir.join(".abook-manifest.json");

    for (name, base) in &srcs {
        let url = format!("{base}/manifest.json");
        if json { eprint!("取 manifest ← {name} ... ") } else { print!("取 manifest ← {name} ... ") };
        if !fetch(&url, &mpath) {
            println!("不可达");
            failures.push(format!("{name}: manifest 下载失败"));
            continue;
        }
        let txt = match std::fs::read_to_string(&mpath) {
            Ok(t) => t,
            Err(e) => {
                println!("读不了: {e}");
                failures.push(format!("{name}: manifest 无法读取"));
                continue;
            }
        };
        match pick_entry(&txt, target) {
            Some((sha, size, ver)) => {
                prog!(json, "{} 条,本平台 {size} 字节", count_entries(&txt));
                entry = Some((sha, size, ver, name.clone()));
                break;
            }
            None => {
                println!("里面没有 {target}");
                failures.push(format!("{name}: manifest 无本平台条目"));
            }
        }
    }
    let _ = std::fs::remove_file(&mpath);

    let (want_sha, want_size, remote_ver, from) = match entry {
        Some(e) => e,
        None => {
            // 显式指定源时不提"回退到 GitHub"，那条路已被主动关掉。
            let hint = if explicit {
                "指定了 --url 就只认这个源，不会回退到公网。请确认目录/基地址存在且含 manifest.json 与 <triple>/abook。"
            } else {
                "1) 直接指定镜像: abook upgrade --url <基地址>\n  2) 手取 https://raw.githubusercontent.com/REPO/downloads/TARGET/abook 放进安装目录\n  3) 若是私有仓库, 设置 GITHUB_TOKEN 走 API 路径"
            };
            return blocked(format!(
                "取不到可用的 manifest。\n  逐条结果:\n    {}\n可行做法:\n  {}",
                failures.join("\n    "),
                hint.replace("REPO", &repo).replace("TARGET", target)
            ))
        }
    };

    // 版本比较只在两端都拿到版本号时做。老 manifest 没有 version 字段,那种情况
    // 不能推断"远端更新"或"已最新",只能老实下载一份再比。
    if !remote_ver.is_empty() && !force {
        let (r, l) = (ver_tuple(&remote_ver), ver_tuple(crate::VERSION));
        // "已最新"与"本地更新"也是检查结论，`--json` 时同样要给 JSON。
        // 这两条早退原本直接打印人话，于是 `upgrade --check --json` 在最新时
        // 输出人话、在有新版时输出 JSON——同一个命令的两种输出形态取决于远端
        // 状态，调用方无法预先知道该按哪种解析。
        if r == l || r < l {
            let state = if r == l { "current" } else { "older" };
            emit_check_result(json, os, arch, target, &remote_ver, want_size, state, &from, &install_to);
            if state == "current" {
                prog!(json, "");
                prog!(json, "已是最新 ({})", crate::VERSION);
            } else {
                prog!(json, "");
                prog!(
                    json,
                    "本地 {} 比远端 {remote_ver} 还新(可能是自己编的),不覆盖。要强制改用 --force",
                    crate::VERSION
                );
            }
            let _ = std::fs::remove_file(&part);
            if !json {
                return Ok(0);
            }
            return Ok(0);
        }
        prog!(json, "远端 {remote_ver}(来自 {from})");
    } else if remote_ver.is_empty() && !force {
        // manifest 里没有 version(旧版 CI 生成的)。此时"更新"与"已最新"都无从
        // 判断,唯一诚实的说法是承认不知道,再照流程下载一次按实得版本决定。
        println!("manifest 未声明版本,无法预判新旧;将完整下载后按实得版本判断");
    }

    if check_only {
        // 三个状态分开报，脚本据此决定要不要真的去装。
        let state = if remote_ver.is_empty() {
            "unknown"
        } else if ver_tuple(&remote_ver) > ver_tuple(crate::VERSION) {
            "newer"
        } else if ver_tuple(&remote_ver) == ver_tuple(crate::VERSION) {
            "current"
        } else {
            "older"
        };
        // `--json` 就必须是 JSON。之前这条路径直接 return 人类可读文本：
        // 会联网、会退出 0，但输出不能被解析——对脚本而言这是最坏的组合，
        // 因为它在 shell 里看着"成功"了，只是字段取不到。
        emit_check_result(json, os, arch, target, &remote_ver, want_size, state, &from, &install_to);
        if json {
            let _ = std::fs::remove_file(&part);
            return Ok(0);
        }
        println!();
        match state {
            "unknown" => {
                println!("远端 {want_size} 字节,但 manifest 未声明版本,无法判断是否比当前新。");
                println!("去掉 --check 即安装(会先校验 sha256 与自检)");
            }
            "newer" => println!("有新版本 {remote_ver},{want_size} 字节。去掉 --check 即安装"),
            "current" => println!("已是最新 ({})", crate::VERSION),
            _ => println!(
                "本地 {} 比远端 {remote_ver} 还新(可能是自己编的),不覆盖。要强制改用 --force",
                crate::VERSION
            ),
        }
        let _ = std::fs::remove_file(&part);
        return Ok(0);
    }

    // 下载。先试拿到 manifest 的那一家,再按原顺序回退——manifest 能通不代表
    // 大文件也通,反过来也一样。
    // 与 manifest 同一张表,不再单独插 --url(已在上面最前面)。
    let bases: Vec<(String, String)> = srcs.clone();

    let mut got = false;
    let mut used = String::new();
    for (name, base) in &bases {
        let url = format!("{base}/{target}/{exe}");
        if json { eprint!("下载 ← {name} ... ") } else { print!("下载 ← {name} ... ") };
        if !fetch(&url, &part) {
            println!("不可达");
            failures.push(format!("{name}: 文件下载失败"));
            continue;
        }
        let bytes = match std::fs::read(&part) {
            Ok(b) => b,
            Err(e) => {
                println!("读不了: {e}");
                failures.push(format!("{name}: 下载内容无法读取"));
                continue;
            }
        };
        if !looks_exec(&bytes) {
            println!("不是可执行文件({} 字节)", bytes.len());
            failures.push(format!("{name}: 内容不是可执行文件"));
            let _ = std::fs::remove_file(&part);
            continue;
        }
        // sha256 与 manifest 对账。只比头的四个字节会让"下到一半"和"内容被代理
        // 改写"都过关,而这个文件接下来要覆盖用户每天在用的二进制。
        if !want_sha.is_empty() {
            let got_sha = sh::digest_hex(&bytes);
            if !got_sha.eq_ignore_ascii_case(&want_sha) {
                println!("校验不符");
                failures.push(format!(
                    "{name}: sha256 不符(期望 {} 实得 {})",
                    &want_sha[..8.min(want_sha.len())],
                    &got_sha[..8.min(got_sha.len())]
                ));
                let _ = std::fs::remove_file(&part);
                continue;
            }
            println!("校验通过 {} 字节", bytes.len());
        } else {
            println!("{} 字节(无校验码可比)", bytes.len());
        }
        got = true;
        used = format!("{name} {url}");
        break;
    }

    if !got {
        let _ = std::fs::remove_file(&part);
        return blocked(format!(
            "下载失败或校验不过。\n  目标 {target}\n  逐条结果:\n    {}\n  安装目录 {}",
            failures.join("\n    "),
            dir.display()
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| EE::Io(format!("设不了可执行位: {e}")))?;
    }

    // 装之前先让这份新文件自己报一次版本。它要覆盖的是用户正在用的可执行文件,
    // "名字对了"不足以证明它在这个架构上真能跑。
    match read_version_of(&part) {
        Some(v) => {
            if !remote_ver.is_empty() && ver_tuple(&v) != ver_tuple(&remote_ver) {
                let _ = std::fs::remove_file(&part);
                return blocked(format!(
                    "下来的文件版本 {v} 与 manifest 声明的 {remote_ver} 不一致,拒绝安装(可能源不同步)。"
                ));
            }
            println!("新文件自报版本 {v}");
        }
        None => {
            let _ = std::fs::remove_file(&part);
            return blocked(format!(
                "下来的文件跑不起来(架构不匹配?),已丢弃。本机 {os}/{arch} → {target}"
            ));
        }
    }

    ensure_dir(&dir)?;

    // 两步换位:先把在用的挪到 .old,再把新文件挪到位。
    // 不用"直接覆盖"是因为 Windows 上正在运行的文件不可删改;而两步法在 Unix
    // 上同样正确,失败时能原路退回去。
    //
    // 这里有三件事必须分清,否则会造出"机器上一份 abook 都不剩"的后果:
    //
    //   1. 安装目标不存在，不等于"本来就没有旧文件"。上一轮 upgrade 若在
    //      "已把旧文件改名成 .old、还没把新文件挪到位"之间被打断（Ctrl-C、
    //      断电、OOM），磁盘上就是 install_to 缺失、`.old` 是唯一一份 abook。
    //      旧代码在动手前无条件删掉 `.old`，于是那份唯一的东西被自己删了；
    //      接着 exists() 为假 → 判定"没有可回滚的文件" → 自检失败删掉新文件
    //      → 目录空了，还打印"原本没有可回滚的文件"（这当然是假的）。
    //
    //   2. "改名失败"必须与"没有旧文件"分开。压成同一个 false 的后果是：
    //      改名其实失败了，旧文件还在原地，之后却走进"没有可回滚"分支。
    //
    //   3. 回滚本身失败必须如实说。原样吞掉 rename 的结果、照样打印"已回滚到
    //      原文件"，会让人停止补救——而他手上已经没有能用的可执行文件了。
    let backup = dir.join(format!(".{exe}.old"));
    let had_install = install_to.exists();
    let had_backup = backup.exists();

    if !had_install && had_backup {
        // 上一轮被打断的现场：把 .old 认作"原来的文件"，先复位回原位。
        // 复位不了就直接停，不要在唯一一份可执行文件上继续动手。
        std::fs::rename(&backup, &install_to).map_err(|e| {
            EE::Io(format!(
                "发现上次升级留下的 {}，但无法复位成 {}: {e}\n\
                 这是当前唯一的可执行文件，已停止以免把它弄丢。请手工执行:\n  mv {} {}",
                backup.display(),
                install_to.display(),
                backup.display(),
                install_to.display()
            ))
        })?;
        println!("发现上次升级中断的残留，已复位 {}", install_to.display());
    } else if had_install && had_backup {
        // 正常情况：旧文件还在原位，那份 .old 是上一轮没清理掉的，可以删。
        let _ = std::fs::remove_file(&backup);
    }

    let moved_old = if install_to.exists() {
        match std::fs::rename(&install_to, &backup) {
            Ok(()) => true,
            Err(e) => {
                // 旧文件还在原位、新文件还在 .upgrade：什么也没破坏，如实报错即可。
                let _ = std::fs::remove_file(&part);
                return Err(EE::Io(format!(
                    "无法把现有文件移开: {e}\n  {} 未被改动，仍是可用状态。",
                    install_to.display()
                )));
            }
        }
    } else {
        false
    };

    if let Err(e) = std::fs::rename(&part, &install_to) {
        // 退回去,别把用户留在一个"原来的没了、新的没装上"的状态里。
        let rolled = if moved_old {
            std::fs::rename(&backup, &install_to).is_ok()
        } else {
            false
        };
        let _ = std::fs::remove_file(&part);
        let tail = if rolled {
            "(已回滚到原文件)".to_string()
        } else {
            // 不能声称回滚了。把现场原样说清楚，并给出可自查的路径。
            format!(
                "(回滚失败，原文件在 {}，请手工改名回 {})",
                backup.display(),
                install_to.display()
            )
        };
        return Err(EE::Io(format!("无法写入 {}: {e}{tail}", install_to.display())));
    }

    // 装好还不算完:跑它自己的自检。一个能装但自检不过的 release 是坏 release,
    // 这种情况必须报失败并退回去,而不是留一份坏的覆盖好的。
    let mut verified = false;
    if let Ok(o) = Command::new(&install_to).arg("selftest").output() {
        if o.status.success() {
            let last = String::from_utf8_lossy(&o.stdout)
                .lines()
                .last()
                .unwrap_or("")
                .trim()
                .to_string();
            // selftest 末行本身就以"自检"开头,再加前缀会变成"自检 自检 …"
            let head = if last.starts_with("自检") { last.to_string() } else { format!("自检 {last}") };
            println!("{head}");
            verified = true;
        }
    }

    if !verified {
        let _ = std::fs::remove_file(&install_to);
        if moved_old {
            match std::fs::rename(&backup, &install_to) {
                Ok(()) => println!("新版本自检未通过,已回滚到原文件"),
                Err(e) => {
                    // 这里说"已回滚"就是在骗人,而此刻用户手上的 abook 已经
                    // 被删掉了,他必须知道去哪找回来。
                    eprintln!(
                        "新版本自检未通过，且回滚失败: {e}\n\
                         原文件仍在 {}，请手工改名回 {}",
                        backup.display(),
                        install_to.display()
                    );
                    return Ok(5);
                }
            }
        } else {
            eprintln!("新版本自检未通过,已删除");
        }
        return Ok(5);
    }

    let _ = std::fs::remove_file(&backup);
    println!();
    println!(
        "已更新 {} → {}(来源 {used})",
        crate::VERSION,
        read_version_of(&install_to).unwrap_or_else(|| "?".to_string())
    );

    // 换的是"正在跑的那个文件",所以只要原路径在 PATH 上,升级后立刻生效,不需要
    // 再教用户改 PATH。这里只在真的不在 PATH 上时才提。
    let on_path = path_dirs()
        .iter()
        .any(|d| Path::new(d).join(exe).exists());
    if !on_path {
        println!();
        println!("这个路径不在 PATH 上,把那行加入 shell 配置后重开终端:");
        println!("  export PATH=\"{}:$PATH\"", dir.display());
    }
    Ok(0)
}

fn count_entries(manifest: &str) -> usize {
    // 数 `"target"` 这个键本身，不带冒号——带上就把空白写死了，
    // 换一种排版就数成 0 条，而 0 条会被读成"manifest 是空的"。
    manifest.matches("\"target\"").count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 版本比较最容易错的地方:字符串比较会把 1.6.9 判成比 1.6.15 新,而表现是
    /// "永远说已是最新",没有报错可循。
    /// 同一份 manifest 换个工具打印，语义完全一样，工具必须都认。
    ///
    /// 这条钉住的是一个真实缺陷：解析曾把 `"target":"x"` 这种紧凑写法写死，
    /// CI 恰好生成紧凑格式，于是一直工作。任何人用 `jq` 或 `json.dumps` 重新
    /// 格式化（冒号后多一个空格），查找立刻落空，而报出来的话是"这个源没有你
    /// 要的架构"——把 JSON 解析问题说成了"没有这个架构"。
    ///
    /// 三处都写死过：target 查找、条目字段提取、顶层 version 提取。改一处漏
    /// 另一处的表现还各不相同（分别是"没有该架构"和"manifest 未声明版本"），
    /// 所以三条一起测。
    #[test]
    fn manifest_parsing_ignores_formatting_whitespace() {
        let compact = r#"{"schema":"abook-downloads/1","version":"1.7.0","files":[
{"target":"x86_64-unknown-linux-musl","os":"linux","arch":"x86_64","libc":"musl","file":"x86_64-unknown-linux-musl/abook","size":123,"sha256":"abc"},
{"target":"aarch64-apple-darwin","os":"macos","arch":"aarch64","libc":"libSystem","file":"aarch64-apple-darwin/abook","size":456,"sha256":"def"}]}"#;
        let spaced = r#"{
  "schema": "abook-downloads/1",
  "version": "1.7.0",
  "files": [
    {
      "target": "x86_64-unknown-linux-musl",
      "os": "linux",
      "arch": "x86_64",
      "libc": "musl",
      "file": "x86_64-unknown-linux-musl/abook",
      "size": 123,
      "sha256": "abc"
    }
  ]
}"#;
        for (label, m) in [("紧凑", compact), ("带空格", spaced)] {
            let (sha, size, ver) = pick_entry(m, "x86_64-unknown-linux-musl")
                .unwrap_or_else(|| panic!("{label} 排版读不出条目"));
            assert_eq!(sha, "abc", "{label}: sha");
            assert_eq!(size, 123, "{label}: size");
            assert_eq!(ver, "1.7.0", "{label}: version");
        }
        // 条目数同样不能把空白写死：数成 0 会被读成"manifest 是空的"。
        assert_eq!(count_entries(compact), 2);
        assert_eq!(count_entries(spaced), 1);
        // 不存在的目标必须是 None，而不是回落到第一条。
        assert!(pick_entry(spaced, "riscv64gc-unknown-linux-gnu").is_none());
    }

    /// 显式指定的源不得回退。
    ///
    /// 这条钉住的是"假验证"：`--url` 指向不可达镜像时，若后面还挂着五个公网源，
    /// GitHub 会接上并成功，最终打印"已是最新"。用户以为在验证内网镜像，实际
    /// 验证的永远是公网，而且屏幕上没有任何提示说明源被换掉了。
    ///
    /// 显式指定源的语义就是"要么它行，要么失败"。
    #[test]
    fn explicit_url_never_falls_back_to_public_sources() {
        let only = pick_sources("Sharl210/abook", Some("https://mirror.internal/abook/"));
        assert_eq!(only.len(), 1, "显式指定源时不该再有别的源: {only:?}");
        assert_eq!(only[0].0, "--url");
        // 末尾斜杠要归一，否则拼出来是 `...//manifest.json`。
        assert_eq!(only[0].1, "https://mirror.internal/abook");

        // 本地目录形态同样只留一条。
        let local = pick_sources("Sharl210/abook", Some("/tmp/mirror"));
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].1, "/tmp/mirror");

        // 没指定时保留完整回退链，且第一条是 GitHub。
        let all = pick_sources("Sharl210/abook", None);
        assert_eq!(all.len(), 5, "{all:?}");
        assert_eq!(all[0].0, "GitHub");
    }

    #[test]
    fn version_ordering_is_numeric_not_lexical() {
        assert!(ver_tuple("1.6.15") > ver_tuple("1.6.9"));
        assert!(ver_tuple("1.7.0") > ver_tuple("1.6.99"));
        assert_eq!(ver_tuple("1.6.15"), ver_tuple("v1.6.15"));
        assert_eq!(ver_tuple("1.6.15-rc1"), ver_tuple("1.6.15"));
        assert_eq!(ver_tuple("1.6.15+build7"), ver_tuple("1.6.15"));
        assert!(ver_tuple("2.0") > ver_tuple("1.99.99"));
    }

    #[test]
    fn manifest_entry_is_picked_by_exact_triple() {
        let m = r#"{"schema":"abook-downloads/1","version":"1.7.0","files":[
{"target":"x86_64-unknown-linux-musl","file":"x86_64-unknown-linux-musl/abook","size":1617896,"sha256":"aaaa"},
{"target":"aarch64-apple-darwin","file":"aarch64-apple-darwin/abook","size":1104816,"sha256":"bbbb"}]}"#;
        let (sha, size, ver) = pick_entry(m, "x86_64-unknown-linux-musl").unwrap();
        assert_eq!(sha, "aaaa");
        assert_eq!(size, 1617896);
        assert_eq!(ver, "1.7.0");
        // 前缀相同的三元组不能被误配到别的架构上
        assert!(pick_entry(m, "aarch64-unknown-linux-musl").is_none());
        assert_eq!(count_entries(m), 2);
    }

    #[test]
    fn html_from_a_hijacked_mirror_is_not_an_executable() {
        let html = b"<!DOCTYPE html><html><head><title>404</title></head><body>not found</body></html>";
        assert!(!looks_exec(html));
        assert!(!looks_exec(b"short"));
        let mut elf = vec![0x7f, b'E', b'L', b'F'];
        elf.resize(80, 0);
        assert!(looks_exec(&elf));
    }
}
