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
    let key = format!("\"target\":\"{}\"", target);
    let at = manifest.find(&key)?;
    let tail = &manifest[at..];
    let end = tail.find('}')?;
    let obj = &tail[..end];
    // 字符串字段。`size` 不走这里——它在 manifest 里是裸数字(`"size":1617896`),
    // 加引号去找会静默读成 0,而 0 在"跳过下载"和"校验"两处都被当成合法值。
    let sfield = |name: &str| -> Option<String> {
        let k = format!("\"{}\":\"", name);
        let s = obj.find(&k)? + k.len();
        let e = obj[s..].find('"')? + s;
        Some(obj[s..e].to_string())
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
    let version = {
        let k = "\"version\":\"";
        manifest
            .find(k)
            .and_then(|s| {
                let s = s + k.len();
                manifest[s..].find('"').map(|e| manifest[s..s + e].to_string())
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

    if a.has("dry-run") || a.has("json") {
        let srcs = sources(&repo);
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

    println!("当前 {} ({os}/{arch})", crate::VERSION);
    println!("目标 {target}");
    println!("安装位置 {}", install_to.display());
    println!();

    // 临时文件放在目标同目录:跨文件系统 rename 会退化成"复制+删除",那样中途
    // 中断就会留下半个可执行文件。`.part` 前缀用点开头,不会被执行。
    let part = dir.join(format!(".{exe}.upgrade"));
    let _ = std::fs::remove_file(&part);

    // `--url` 必须同时作用于 manifest 与文件。只作用一半会出现"文件从镜像取、
    // manifest 从 GitHub 取",而镜像存在的理由恰恰是 GitHub 不通——那时它两个都
    // 取不到,却报得像只有文件失败。
    let mut srcs = sources(&repo);
    if let Some(u) = a.get("url") {
        srcs.insert(0, ("--url".to_string(), u.trim_end_matches('/').to_string()));
    }
    let mut failures: Vec<String> = Vec::new();
    let mut entry: Option<(String, u64, String, String)> = None; // sha,size,ver,源名
    let mpath = dir.join(".abook-manifest.json");

    for (name, base) in &srcs {
        let url = format!("{base}/manifest.json");
        print!("取 manifest ← {name} ... ");
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
                println!("{} 条,本平台 {size} 字节", count_entries(&txt));
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
            return blocked(format!(
                "所有源都取不到可用的 manifest。\n  逐条结果:\n    {}\n可行做法:\n  1) 直接指定镜像: abook upgrade --url <基地址>\n  2) 手取 https://raw.githubusercontent.com/{repo}/downloads/{target}/abook 放进 {}\n  3) 若是私有仓库, 设置 GITHUB_TOKEN 走 API 路径",
                failures.join("\n    "),
                dir.display()
            ))
        }
    };

    // 版本比较只在两端都拿到版本号时做。老 manifest 没有 version 字段,那种情况
    // 不能推断"远端更新"或"已最新",只能老实下载一份再比。
    if !remote_ver.is_empty() && !force {
        let (r, l) = (ver_tuple(&remote_ver), ver_tuple(crate::VERSION));
        if r == l {
            println!();
            println!("已是最新 ({})", crate::VERSION);
            let _ = std::fs::remove_file(&part);
            return Ok(0);
        }
        if r < l {
            println!();
            println!(
                "本地 {} 比远端 {remote_ver} 还新(可能是自己编的),不覆盖。要强制改用 --force",
                crate::VERSION
            );
            let _ = std::fs::remove_file(&part);
            return Ok(0);
        }
        println!("远端 {remote_ver}(来自 {from})");
    } else if remote_ver.is_empty() && !force {
        // manifest 里没有 version(旧版 CI 生成的)。此时"更新"与"已最新"都无从
        // 判断,唯一诚实的说法是承认不知道,再照流程下载一次按实得版本决定。
        println!("manifest 未声明版本,无法预判新旧;将完整下载后按实得版本判断");
    }

    if check_only {
        println!();
        if remote_ver.is_empty() {
            println!("远端 {want_size} 字节,但 manifest 未声明版本,无法判断是否比当前新。");
            println!("去掉 --check 即安装(会先校验 sha256 与自检)");
        } else {
            println!("有新版本 {remote_ver},{want_size} 字节。去掉 --check 即安装");
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
        print!("下载 ← {name} ... ");
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
    let backup = dir.join(format!(".{exe}.old"));
    let _ = std::fs::remove_file(&backup);
    let moved_old = if install_to.exists() {
        std::fs::rename(&install_to, &backup).is_ok()
    } else {
        false
    };

    if let Err(e) = std::fs::rename(&part, &install_to) {
        // 退回去,别把用户留在一个"原来的没了、新的没装上"的状态里。
        if moved_old {
            let _ = std::fs::rename(&backup, &install_to);
        }
        let _ = std::fs::remove_file(&part);
        return Err(EE::Io(format!(
            "无法写入 {}: {e}(已回滚到原文件)",
            install_to.display()
        )));
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
        if moved_old {
            let _ = std::fs::remove_file(&install_to);
            let _ = std::fs::rename(&backup, &install_to);
            println!("新版本自检未通过,已回滚到原文件");
        } else {
            let _ = std::fs::remove_file(&install_to);
            println!("新版本自检未通过,已删除(原本没有可回滚的文件)");
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
    manifest.matches("\"target\":").count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 版本比较最容易错的地方:字符串比较会把 1.6.9 判成比 1.6.15 新,而表现是
    /// "永远说已是最新",没有报错可循。
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
