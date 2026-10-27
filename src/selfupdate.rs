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
//! 3. **替换的是当前正在运行的那个文件**,不是硬编码的某条目录。
//!    二进制可能被装在别处(库根、或是包管理器管的路径),猜
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

/// `pick_entry` 为什么失败——三类，下一步动作完全不同。
///
/// 原先只有一个 `Option`：所有失败都从同一个 `None` 出来，调用方只能说"里面没有
/// {target}"。于是"manifest 是一页 HTML 劫持页""文件是空的""JSON 合法但没有你的
/// 架构"报出来一模一样，而这三件事的下一步是相反的：前两个要换源或查代理，最后
/// 一个才叫"这个源不提供你的平台"。把 A 说成 B，用户会拿着正确的源一直换下去。
#[derive(Debug)]
enum EntryError {
    /// manifest 根本不是 JSON：空文件、劫持页、被截断的响应。带诊断文本。
    NotJson(String),
    /// JSON 合法，但 files 里没有本平台的三元组。
    NoSuchTarget,
    /// 有本平台条目，但条目字段缺了或为 0。带诊断文本。
    BadEntry(String),
}

/// manifest 的形态校验：不是一份 JSON 下载清单就当场说清楚。
///
/// 这一步如果省掉，`pick_entry` 在 HTML 页面、空文件、半截响应上都只能得出"找不到
/// target"，而上层会把它说成"这个源没有你的架构"——排查方向直接被引到"换源"上，
/// 而真实原因在劫持页或代理改写。三种坏输入都真实出现过，所以三条都要单独说。
fn manifest_shape_error(txt: &str) -> Option<String> {
    // 带上前 80 个字符的样子：空文件、HTML、被截断的 JSON 在第 0 眼就能看出来，
    // 而只报"不是 JSON"还得让用户自己去 cat 一遍才知道是哪种。
    let head: String = {
        let mut s: String = txt.chars().take(80).collect();
        s = s.replace(['\n', '\r', '\t'], " ");
        if txt.chars().count() > 80 {
            s.push('…');
        }
        s
    };
    if txt.trim().is_empty() {
        return Some(
            "manifest 是空文件（0 字节）：源返回了空内容，不是\"没有本平台条目\"。\
             先确认 --url/基地址与路径对不对，再换一个源；每个源都这样就是地址或代理的问题。"
                .to_string(),
        );
    }
    if !txt.contains('{') {
        return Some(format!(
            "manifest 内容不是 JSON（整份里没有 {{），开头是「{head}」：\
             多半是劫持页、502 错误页或登录页。这不是\"没有本平台条目\"，\
             换源或改用正确的 --url；若每个源都一样，检查代理是否在改写响应正文。"
        ));
    }
    if !txt.contains("\"files\"") {
        return Some(format!(
            "manifest 内容不是 JSON 下载清单（有 {{ 但没有 \"files\"），开头是「{head}」：\
             可能被截断、也可能是另一种格式的响应。重新取一次，或换源；持续如此要报给发布端。"
        ));
    }
    None
}

/// 从 manifest 里抠出本平台那条记录。返回 (sha256, size, version)。
///
/// 手写解析而不是引一个 JSON 依赖:整个工具零外部依赖是既有约定,而这里只需要
/// 一个对象里的一层数组。找的是 `"target"` 那个键,再取出它的字符串值跟上三目
/// 三元组;命中后在它之后读到下一个 `}` 为止。
fn pick_entry(manifest: &str, target: &str) -> Result<(String, u64, String), EntryError> {
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
    if let Some(why) = manifest_shape_error(manifest) {
        return Err(EntryError::NotJson(why));
    }
    let key = "\"target\"";
    let mut from = 0usize;
    while let Some(rel) = manifest[from..].find(key) {
        let at = rel + from;
        from = at + key.len();
        // 取 `"target"` 之后、下一个 `"` 之前的值，逐段跳过空白。
        //
        // 这里用 `continue` 而不是 `?`：某个键后面恰好不是字符串(多一层嵌套、被
        // 截断)时，原先的 `?` 会让整次查找立刻放弃，连后面本该命中的条目也读不到，
        // 最终又变成"没有你的架构"。跳过这一次继续扫，才是"只坏了一条"该有的行为。
        let Some(rest) = manifest[at + key.len()..].trim_start().strip_prefix(':') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('"') else {
            continue;
        };
        let Some(end) = rest.find('"') else {
            continue;
        };
        if &rest[..end] != target {
            continue;
        }
        return finish_entry(manifest, at, target);
    }
    Err(EntryError::NoSuchTarget)
}

/// 条目已经定位到 `"target"` 这个下标，把它的字段读出来。
///
/// 字符串字段与数字字段都得容忍冒号两侧的空白：`sfield` 早就改了，`nfield` 还留着
/// `"size":` 这种写死形态，于是 `"size" : 123`（冒号前一个空格）读不到 size，
/// 整条被当成坏的丢掉，上层报"里面没有 {target}"——又一处"改一处漏一处"。
fn finish_entry(
    manifest: &str,
    at: usize,
    target: &str,
) -> Result<(String, u64, String), EntryError> {
    let tail = &manifest[at..];
    let Some(end) = tail.find('}') else {
        return Err(EntryError::BadEntry(format!(
            "manifest 里 {target} 这一条没有闭合的 }}，文件像是被截断了（下载不全）。\
             重新取一次 manifest；每次都这样就是源在截断响应。"
        )));
    };
    let obj = &tail[..end];
    // `size` 不走 sfield——它在 manifest 里是裸数字(`"size":1617896`)，
    // 加引号去找会静默读成 0，而 0 在"跳过下载"和"校验"两处都被当成合法值。
    let sfield = |name: &str| -> Option<String> {
        let k = format!("\"{}\"", name);
        let at = obj.find(&k)? + k.len();
        let rest = obj[at..].trim_start().strip_prefix(':')?.trim_start();
        let rest = rest.strip_prefix('"')?;
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    };
    let nfield = |name: &str| -> Option<u64> {
        let k = format!("\"{}\"", name);
        let rest = &obj[obj.find(&k)? + k.len()..];
        let rest = rest.trim_start().strip_prefix(':')?.trim_start();
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        // 数字后面紧跟字母说明这个值本身是坏的(`123abc`)。原样截成 123 会让它
        // 拿去跟 sha256 对账，报出来的却是"校验不符"，把 manifest 的错说成
        // 下载内容的错。
        if let Some(next) = rest[digits.len()..].chars().next() {
            if next.is_ascii_alphanumeric() || next == '_' {
                return None;
            }
        }
        digits.parse::<u64>().ok()
    };
    let sha = match sfield("sha256") {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Err(EntryError::BadEntry(format!(
                "manifest 里 {target} 这一条缺 sha256（或值为空）。没有校验码就无从确认下下来的\
                 字节完整，本工具不做无校验安装；请修发布端的 manifest 生成。"
            )))
        }
    };
    let size = match nfield("size") {
        // 0 字节不可能是一份能跑的二进制。这里与"没有该架构"必须分开报：前者是
        // 发布端产物没生成好，后者才是这个源不提供你的平台。
        Some(0) => {
            return Err(EntryError::BadEntry(format!(
                "manifest 里 {target} 这一条的 size 是 0：产物没生成或没上传完整，\
                 这个源暂时装不了。换一个源，或报给发布端重新发一版。"
            )))
        }
        Some(n) => n,
        None => {
            return Err(EntryError::BadEntry(format!(
                "manifest 里 {target} 这一条缺 size 字段（或不是纯数字）：下载前无法确认体积，\
                 本工具不猜。注意这跟\"这个源没有你的架构\"是两回事——条目在，只是字段坏了。"
            )))
        }
    };
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
    Ok((sha, size, version))
}

fn curl() -> bool {
    Command::new("curl")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 一次取文件失败的原因。
///
/// 分四类不是为了措辞好听:四类的下一步动作完全不同。网络类换源、配代理有意义;
/// 本地写不进去(安装目录只读、磁盘满、父目录不存在)换五个源也是同样的失败。
/// 原先这里只返回 `bool`,调用方只能一律说"不可达",于是"我要装的目录不存在"
/// 被说成"这些源都用不了"——用户会一直重试下载,而重试一万次也装不上。
#[derive(Debug, Clone, PartialEq, Eq)]
enum FetchError {
    /// curl 6/7/28：域名解析不了、连不上、连上超时。
    Unreachable(String),
    /// 连上了但没拿到内容：HTTP 4xx/5xx(`-f` 下是 curl 22)、TLS/证书(35/60)。
    Remote(String),
    /// 内容拉得到但写不进本地：curl 23。纯本机问题。
    LocalWrite(String),
    /// `--url` 指向本地目录，而里面没有这个文件。
    LocalSource(String),
}

impl FetchError {
    /// 进度行上的短状态（一行长度，跟在"取 manifest ← 源 ... "后面）。
    fn short(&self) -> &'static str {
        match self {
            FetchError::Unreachable(_) => "连不上",
            FetchError::Remote(_) => "源取不到内容",
            FetchError::LocalWrite(_) => "写不进本地",
            FetchError::LocalSource(_) => "本地源没有这个文件",
        }
    }

    /// 汇总用的完整诊断：发生了什么 + 为什么 + 下一步。
    ///
    /// 三段都要有。只有"失败"两个字时，用户唯一能做的就是重试；写清这是本机
    /// 写盘问题，他才会去看目录权限和磁盘余量。
    fn detail(&self) -> String {
        match self {
            FetchError::Unreachable(d) => format!(
                "连不上这个源（{d}）。可能被墙、DNS 不通或对端超时；\
                 换一个源，或给 curl 配上代理（https_proxy）再试。"
            ),
            FetchError::Remote(d) => format!(
                "连上了但没取到内容（{d}）。确认基地址/路径正确、证书可信；\
                 换一个源通常能绕开单点故障。"
            ),
            FetchError::LocalWrite(d) => format!(
                "内容能拉下来但写不进本地（{d}）。这是本机问题——安装目录只读/不存在、\
                 磁盘满了，换源或重试下载都不会好；先修目录权限或腾出空间。"
            ),
            FetchError::LocalSource(p) => format!(
                "本地目录里没有这个文件（{p}）。确认 --url 指向的镜像目录含 manifest.json 与 \
                 <triple>/abook，或把 --url 换成 http(s) 基地址。"
            ),
        }
    }

}

/// curl 退出码 → 该走哪条下一步。
///
/// 6/7/28 是"到不了对端"，23 是"到得了但对端的东西写不到本地盘"，其余（22 HTTP
/// 错误、35/60 TLS）是"对端不给内容"。把 23 混进网络类，人会对着一个只读目录
/// 反复重试下载——这是这次要修的四种"报错指向错误方向"里最费时间的一种，因为
/// 每一次重试都要等满超时。
fn classify_curl(code: i32, stderr: &str, to: &Path) -> FetchError {
    let raw = stderr
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();
    let raw = if raw.is_empty() {
        format!("curl 退出码 {code}，没有输出")
    } else {
        raw
    };
    match code {
        6 | 7 | 28 => FetchError::Unreachable(raw),
        23 => FetchError::LocalWrite(format!("目标 {}：{raw}", to.display())),
        _ => FetchError::Remote(raw),
    }
}

/// 下载到指定路径。失败原因交给调用方按源逐条汇总。
///
/// 没有 `://` 的地址当作本地目录:内网共享盘、离线机上的 U 盘镜像、以及自己做
/// 镜像验证时都会用到一个裸路径,而 curl 对裸路径只会报"协议不支持"。
fn fetch(url: &str, to: &Path) -> Result<(), FetchError> {
    if !url.contains("://") {
        let from = Path::new(url);
        // 先判源在不在，再复制。反过来的话"源里没这个文件"与"目标父目录不存在"
        // 都是 `fs::copy` 的 NotFound，而这两件事的下一步正好相反：一个去查镜像
        // 内容，一个去查安装目录权限。分不清就只能都报成"不可达"。
        if !from.is_file() {
            return Err(FetchError::LocalSource(from.display().to_string()));
        }
        return std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| FetchError::LocalWrite(format!("复制到 {} 失败: {e}", to.display())));
    }
    // `-fsSL` 里那个 `S` 是关键：`-s` 会连错误正文一起吞掉，只有再加 `S` 才会把
    // "为什么失败"写到 stderr。改 `.status()` 为 `.output()` 是为了把这段 stderr
    // 收进来分类，再作为诊断发给用户——原先 stderr 直接漏到屏幕上是一行裸 curl
    // 错误，而汇总清单里写的是"不可达"，两处说法互相矛盾，人只能信看得懂的那行。
    let out = Command::new("curl")
        // `--connect-timeout` 单独给:被墙的表现是连接阶段挂住而不是收到拒绝,只靠
        // 总超时会让每个源白等满 90 秒×5 个源。
        .args([
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
        .arg(url)
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(classify_curl(
            o.status.code().unwrap_or(-1),
            &String::from_utf8_lossy(&o.stderr),
            to,
        )),
        // curl 都起不来（没装、没执行权限）：这也常被后续说成"源不可达"。
        Err(e) => Err(FetchError::Remote(format!(
            "启动 curl 失败: {e}（先确认本机 curl 可执行）"
        ))),
    }
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

    // 更新目标是"正在跑的这个文件"。不能用 `current_exe` 直接取:
    // Android 上内核 exec 的是 `/apex/com.android.runtime/bin/linker64`,
    // `/proc/self/exe` 于是指向**链接器**而不是我们。照它写回等于往只读系统
    // 分区覆盖链接器,是最坏的一种猜错。`util::self_path` 先读
    // `/proc/self/cmdline` 拿真实路径,并在兜底里显式拒绝链接器名字。
    let cur = crate::util::self_path().ok_or_else(|| {
        EE::Io(
            "拿不到当前程序路径（可能运行在动态链接器下）。用 --dir 显式指定安装目录，例如 `abook upgrade --dir ~/AgentsBook`"
                .to_string(),
        )
    })?;
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
            "本机没有 curl,无法下载。请手动取一份对应架构的产物放进库根。".to_string(),
        );
    }

    let dir = install_to
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(home_dir);

    // 建目录必须排在"往这个目录写临时文件"之前。
    //
    // 原先它在下载之后。`--dir /opt/abook` 指向一个还没建的目录时，最先落地的是
    // manifest 临时文件，curl 以 23（写不进本地）失败，逐条结果全变成"不可达"，
    // 末尾还提示"请确认目录/基地址存在"——把"我要装的目录还不存在、你可以建"
    // 说成了"这些源都用不了"。五条失败加一句网络建议，用户只会去重试下载。
    ensure_dir(&dir)?;

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
    // 本机侧失败的两种，分开记：写盘失败要查目录权限/磁盘空间，源缺文件要查镜像
    // 内容。合成一个标记的话，尾巴提示会把查镜像的人引去改权限，白折腾。
    let mut local_write_seen = false;
    let mut local_source_seen = false;
    let mut entry: Option<(String, u64, String, String)> = None; // sha,size,ver,源名
    let mpath = dir.join(".abook-manifest.json");

    for (name, base) in &srcs {
        let url = format!("{base}/manifest.json");
        if json { eprint!("取 manifest ← {name} ... ") } else { print!("取 manifest ← {name} ... ") };
        if let Err(e) = fetch(&url, &mpath) {
            // 状态行跟着 `prog!` 走：`--json` 下 stdout 只能有那一个 JSON，
            // 一个"不可达"混进去，调用方的解析器整个失败，且失败点在对方。
            prog!(json, "{}", e.short());
            match &e {
                FetchError::LocalWrite(_) => local_write_seen = true,
                FetchError::LocalSource(_) => local_source_seen = true,
                _ => {}
            }
            failures.push(format!("{name}: manifest {}", e.detail()));
            continue;
        }
        let txt = match std::fs::read_to_string(&mpath) {
            Ok(t) => t,
            Err(e) => {
                prog!(json, "读不了: {e}");
                failures.push(format!("{name}: manifest 无法读取: {e}"));
                continue;
            }
        };
        match pick_entry(&txt, target) {
            Ok((sha, size, ver)) => {
                prog!(json, "{} 条,本平台 {size} 字节", count_entries(&txt));
                entry = Some((sha, size, ver, name.clone()));
                break;
            }
            // 三类失败分开报，因为它们指向完全不同的下一步：不是 JSON → 换源/查代理；
            // 字段坏了 → 报给发布端；真没有这个架构 → 换一个提供该平台的源。
            Err(why) => {
                let (short, detail) = match &why {
                    EntryError::NotJson(d) => ("manifest 不是 JSON", d.clone()),
                    EntryError::BadEntry(d) => ("manifest 条目有问题", d.clone()),
                    EntryError::NoSuchTarget => (
                        "里面没有该架构",
                        format!("manifest 是合法 JSON，但 files 里没有 {target} 这一条：这个源不提供本平台，换一个源。"),
                    ),
                };
                prog!(json, "{short}");
                failures.push(format!("{name}: {detail}"));
            }
        }
    }
    let _ = std::fs::remove_file(&mpath);

    let (want_sha, want_size, remote_ver, from) = match entry {
        Some(e) => e,
        None => {
            // 显式指定源时不提"回退到 GitHub"，那条路已被主动关掉。
            let hint = if explicit {
                "指定了 --url 就只认这个源，不会回退到公网。请确认 --url 指向的源存在，且含 manifest.json 与 <triple>/abook。"
            } else {
                "1) 直接指定镜像: abook upgrade --url <基地址>\n  2) 手取 https://raw.githubusercontent.com/REPO/downloads/TARGET/abook 放进安装目录\n  3) 若是私有仓库, 设置 GITHUB_TOKEN 走 API 路径"
            };
            let mut msg = format!(
                "取不到可用的 manifest。\n  逐条结果:\n    {}\n可行做法:\n  {}",
                failures.join("\n    "),
                hint.replace("REPO", &repo).replace("TARGET", target)
            );
            // 只有真的出现本机侧失败才加这句。一律附带的话，正常"五个源都不通"
            // 的网络故障也会被引去查目录权限，反而增加无效动作。
            if local_write_seen {
                msg.push_str(&format!(
                    "\n注意：上面的失败里有\"写不进本地\"，不是网络问题——换源、重试下载都不会\
                     好转。先确认 {} 可写、磁盘还有空间。",
                    dir.display()
                ));
            }
            if local_source_seen {
                msg.push_str(
                    "\n注意：\"本地源没有这个文件\"同样是本机侧的问题——源那头没有要的东西，\
                     不是连不上。核对 --url 指向的镜像目录内容（manifest.json 与 <triple>/abook）。",
                );
            }
            return blocked(msg);
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
        if let Err(e) = fetch(&url, &part) {
            prog!(json, "{}", e.short());
            match &e {
                FetchError::LocalWrite(_) => local_write_seen = true,
                FetchError::LocalSource(_) => local_source_seen = true,
                _ => {}
            }
            failures.push(format!("{name}: 文件 {}", e.detail()));
            continue;
        }
        let bytes = match std::fs::read(&part) {
            Ok(b) => b,
            Err(e) => {
                prog!(json, "读不了: {e}");
                failures.push(format!("{name}: 下载内容无法读取: {e}"));
                continue;
            }
        };
        if !looks_exec(&bytes) {
            prog!(json, "不是可执行文件({} 字节)", bytes.len());
            failures.push(format!("{name}: 内容不是可执行文件"));
            let _ = std::fs::remove_file(&part);
            continue;
        }
        // sha256 与 manifest 对账。只比头的四个字节会让"下到一半"和"内容被代理
        // 改写"都过关,而这个文件接下来要覆盖用户每天在用的二进制。
        if !want_sha.is_empty() {
            let got_sha = sh::digest_hex(&bytes);
            if !got_sha.eq_ignore_ascii_case(&want_sha) {
                prog!(json, "校验不符");
                failures.push(format!(
                    "{name}: sha256 不符(期望 {} 实得 {})",
                    &want_sha[..8.min(want_sha.len())],
                    &got_sha[..8.min(got_sha.len())]
                ));
                let _ = std::fs::remove_file(&part);
                continue;
            }
            prog!(json, "校验通过 {} 字节", bytes.len());
        } else {
            prog!(json, "{} 字节(无校验码可比)", bytes.len());
        }
        got = true;
        used = format!("{name} {url}");
        break;
    }

    if !got {
        let _ = std::fs::remove_file(&part);
        let mut msg = format!(
            "下载失败或校验不过。\n  目标 {target}\n  逐条结果:\n    {}\n  安装目录 {}",
            failures.join("\n    "),
            dir.display()
        );
        if local_write_seen {
            msg.push_str(
                "\n注意：上面至少一个失败是\"写不进本地\"，不是网络问题——换源、重试下载\
                 都不会好转。先确认安装目录可写、磁盘还有空间。",
            );
        }
        if local_source_seen {
            // 文件这一层的"本地源没有"与网络不通同样要分开：`--url` 指本地镜像时，
            // 这里说的其实是"镜像里没有 <triple>/abook"，去查目录内容才有用。
            msg.push_str(
                "\n注意：上面至少一个失败是\"本地源没有这个文件\"——源那头没放对应的产物，\
                 不是连不上。核对 --url 指向的镜像目录里有没有 <triple>/abook。",
            );
        }
        return blocked(msg);
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

    // 目录在写临时文件之前就建好了（见上面 ensure_dir 的注释），这里不再重复。

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

    // 换的是"正在跑的那个文件",所以升级后立刻生效,不需要再教用户改任何东西。
    //
    // 这里原本还要检查"在不在 PATH 上",不在就打印一行 export。那条建议现在已经
    // 是错的出路:程序与库同目录,路径由它自己决定,把它加进 PATH 只是让两个
    // 不同的 abook 有机会同时存在。只报库根与可执行文件的绝对路径。
    println!();
    println!("库根    {}", crate::util::resolve_root(a).display());
    println!("可执行  {}", install_to.display());
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
                .unwrap_or_else(|e| panic!("{label} 排版读不出条目: {e:?}"));
            assert_eq!(sha, "abc", "{label}: sha");
            assert_eq!(size, 123, "{label}: size");
            assert_eq!(ver, "1.7.0", "{label}: version");
        }
        // 条目数同样不能把空白写死：数成 0 会被读成"manifest 是空的"。
        assert_eq!(count_entries(compact), 2);
        assert_eq!(count_entries(spaced), 1);
        // 不存在的目标必须是 None，而不是回落到第一条。
        assert!(matches!(
            pick_entry(spaced, "riscv64gc-unknown-linux-gnu"),
            Err(EntryError::NoSuchTarget)
        ));
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
        assert!(matches!(
            pick_entry(m, "aarch64-unknown-linux-musl"),
            Err(EntryError::NoSuchTarget)
        ));
        assert_eq!(count_entries(m), 2);
    }


    /// manifest 不是 JSON 时，报的必须不是"没有你的架构"。
    ///
    /// 这条钉住的是一个真实缺陷：`pick_entry` 原来返回 `Option`，空文件、502 劫持页、
    /// 被截断的响应都从同一个 `None` 出来，调用方只能说"里面没有 {target}"。用户照着
    /// 那句话去换源，而问题在劫持页/空响应上——换十个源都是同一页 HTML。
    ///
    /// 所以这里要断言两件事：形态问题必须落进 `NotJson`；合法 JSON 但确实没这个
    /// 架构必须落进 `NoSuchTarget`。两者混在一起，这条修复就白做了。
    #[test]
    fn manifest_that_is_not_json_is_not_reported_as_missing_arch() {
        let target = "x86_64-unknown-linux-musl";
        let html = "<!DOCTYPE html>\n<html><head><title>502 Bad Gateway</title>\
                    </head><body>nginx</body></html>";
        let cases = [
            ("劫持页", html),
            ("空文件", ""),
            ("只有空白", "  \n\t "),
            ("截断的 JSON", "{\"schema\":\"abook-downloads/1\""),
        ];
        for (label, txt) in cases {
            match pick_entry(txt, target) {
                Err(EntryError::NotJson(why)) => {
                    assert!(
                        why.contains("不是 JSON") || why.contains("空文件"),
                        "{label}: 诊断要说清是形态问题: {why}"
                    );
                    // 形态问题的诊断不能反过来把人引去换架构。
                    assert!(
                        !why.contains("里面没有") && !why.contains("没有你的架构"),
                        "{label}: 形态问题被说成了没架构: {why}"
                    );
                    if !txt.trim().is_empty() {
                        // 带样本，人才知道是劫持页还是被截断，不用自己去 cat。
                        assert!(why.contains("开头是"), "{label}: 缺内容样本: {why}");
                    }
                }
                other => panic!("{label} 应当报 NotJson，实际 {other:?}"),
            }
        }

        // 对照：合法 JSON、只是没有这个架构。与上面必须可区分。
        let ok = r#"{"version":"1.7.0","files":[{"target":"aarch64-apple-darwin","size":1,"sha256":"aa"}]}"#;
        assert!(matches!(
            pick_entry(ok, target),
            Err(EntryError::NoSuchTarget)
        ));
    }

    /// 条目字段坏了（缺 size / size 为 0 / 缺 sha256）不能报成"没有该架构"。
    ///
    /// 顺带钉住 `nfield` 的空白容忍：`"size" : 123` 是同一个 JSON 的合法写法，而
    /// `sfield` 早就改成容忍空白、`nfield` 还写着 `"size":`——改一处漏一处的典型。
    /// 漏掉的那处表现是"整条被当坏条目丢掉"，上层再说成没有这个架构。
    #[test]
    fn entry_field_problems_are_not_reported_as_missing_arch() {
        let target = "x86_64-unknown-linux-musl";

        // 冒号前有空格：必须读得到，否则格式化过的 manifest 就装不了。
        let spaced_colon = r#"{"version":"1.7.0","files":[{"target":"x86_64-unknown-linux-musl","sha256":"abc","size" : 1617896}]}"#;
        let (sha, size, ver) =
            pick_entry(spaced_colon, target).expect("`\"size\" : N` 也是合法 JSON，必须读得出来");
        assert_eq!(sha, "abc");
        assert_eq!(size, 1617896);
        assert_eq!(ver, "1.7.0");

        // 缺 size：条目在，只是字段没有 → BadEntry，且提示要提到 size。
        let no_size =
            r#"{"version":"1.7.0","files":[{"target":"x86_64-unknown-linux-musl","sha256":"abc"}]}"#;
        match pick_entry(no_size, target) {
            Err(EntryError::BadEntry(why)) => {
                assert!(why.contains("size"), "要指出缺哪个字段: {why}");
                assert!(why.contains("字段"), "要说清是字段问题: {why}");
            }
            other => panic!("缺 size 应当是 BadEntry，实际 {other:?}"),
        }

        // size 为 0：产物没生成好，也不是"没有该架构"。
        let zero = r#"{"version":"1.7.0","files":[{"target":"x86_64-unknown-linux-musl","size":0,"sha256":"abc"}]}"#;
        match pick_entry(zero, target) {
            Err(EntryError::BadEntry(why)) => assert!(why.contains("0"), "{why}"),
            other => panic!("size=0 应当是 BadEntry，实际 {other:?}"),
        }

        // 缺 sha256：没有校验码就不该装，提示要说出这一点而不是"没架构"。
        let no_sha =
            r#"{"version":"1.7.0","files":[{"target":"x86_64-unknown-linux-musl","size":1617896}]}"#;
        match pick_entry(no_sha, target) {
            Err(EntryError::BadEntry(why)) => assert!(why.contains("sha256"), "{why}"),
            other => panic!("缺 sha256 应当是 BadEntry，实际 {other:?}"),
        }

        // 数字后面跟字母（`123abc`）是坏值：不能截成 123 拿去和 sha 对账，
        // 那会把 manifest 的错报成"校验不符"。
        let bad_num = r#"{"version":"1.7.0","files":[{"target":"x86_64-unknown-linux-musl","size":123abc,"sha256":"abc"}]}"#;
        assert!(matches!(
            pick_entry(bad_num, target),
            Err(EntryError::BadEntry(_))
        ));
    }

    /// curl 的失败码要按"下一步该做什么"分类。
    ///
    /// 关键是 23：它表示"内容下来了但写不进本地盘"（目录只读、磁盘满、父目录不存在）。
    /// 归到网络类，用户会一直重试下载——每次都要等满超时，而磁盘不会因此变空。
    /// 6/7/28 才是"到不了对端"，那才值得换源。
    #[test]
    fn curl_failures_are_classified_by_next_action() {
        let to = Path::new("/var/lib/abook/.abook-manifest.json");
        for code in [6, 7, 28] {
            assert!(
                matches!(
                    classify_curl(code, "curl: (28) Connection timed out after 6001 ms", to),
                    FetchError::Unreachable(_)
                ),
                "curl {code} 属于连不上"
            );
        }

        let e = classify_curl(
            23,
            "curl: (23) client returned ERROR on write of 259 bytes",
            to,
        );
        match &e {
            FetchError::LocalWrite(d) => assert!(
                d.contains(".abook-manifest.json"),
                "要说清写不进哪个文件: {d}"
            ),
            other => panic!("curl 23 必须归为本地写失败，实际 {other:?}"),
        }
        assert!(
            !e.detail().contains("换一个源"),
            "本地写失败不能建议换源: {}",
            e.detail()
        );
        assert!(
            e.detail().contains("磁盘"),
            "要给出真实下一步（查目录/磁盘）: {}",
            e.detail()
        );

        // HTTP 404 / TLS：连上了但对端不给内容。换源有意义，与 23 不同。
        for code in [22, 35, 60] {
            assert!(
                matches!(
                    classify_curl(code, "curl: (22) The requested URL returned error: 404", to),
                    FetchError::Remote(_)
                ),
                "curl {code} 属于对端不给内容"
            );
        }

        // stderr 为空也不能退化成一句没有信息的"失败"。
        assert!(classify_curl(7, "", to).detail().contains('7'));
    }

    /// 本地源缺文件 / 目标写不进去，都不能被说成"不可达"。
    ///
    /// `fetch` 原来只回 `bool`，调用方一律写"不可达"。而 `--url` 指本地目录时，
    /// "源里没有这个文件"和"安装目录写不进去"都会落到那一句上，把两个方向的排查
    /// （查镜像内容 / 查目录权限）合成一句谁也用不上的网络结论。
    #[test]
    fn fetch_tells_local_problems_apart_from_network_ones() {
        let tmp = std::env::temp_dir().join(format!("abook-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // 本地目录源里没有这个文件
        let missing = tmp.join("no-such-manifest.json");
        let dest = tmp.join("dest.json");
        let e = fetch(missing.to_str().unwrap(), &dest).expect_err("源里没这个文件必须报错");
        assert!(matches!(e, FetchError::LocalSource(_)), "{e:?}");
        let d = e.detail();
        assert!(d.contains("本地目录"), "要说清这是本地源: {d}");
        assert!(!d.contains("不可达"), "本地源缺文件不能说成不可达: {d}");

        // 目标写成目录（只读/磁盘满时 curl 也是这条 23 路）：本机问题
        let src = tmp.join("src.json");
        std::fs::write(&src, b"{}").unwrap();
        let as_dir = tmp.join("dest-is-a-dir");
        std::fs::create_dir_all(&as_dir).unwrap();
        let e2 = fetch(src.to_str().unwrap(), &as_dir).expect_err("写到目录上必须失败");
        assert!(matches!(e2, FetchError::LocalWrite(_)), "{e2:?}");
        assert!(e2.detail().contains("本机问题"), "{}", e2.detail());
        assert!(
            matches!(e2, FetchError::LocalWrite(_)),
            "写盘失败要能被汇总单独识别出来（它和\"源里缺文件\"的下一步不同）"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `--dir` 指向还没建的目录时，目录必须在下第一个临时文件之前建好。
    ///
    /// 钉住的是顺序，不是措辞：原来 `ensure_dir` 排在下载之后，于是第一个落地的是
    /// manifest 临时文件，curl 以 23 失败，五条结果全变成"不可达"、末尾还提示
    /// "请确认目录/基地址存在"——把"你要装的目录还不存在（我能建）"说成了"这些源
    /// 都用不了"。磁盘上目录有没有被建出来，就是那条顺序的可观察证据。
    #[test]
    fn upgrade_creates_missing_install_dir_before_downloading() {
        let base = std::env::temp_dir().join(format!("abook-upg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let install_dir = base.join("nested/install");
        // 一个存在的空镜像：源本身没问题，缺的是 manifest。这样失败就一定发生在
        // "写临时文件"这一步，而不会牵扯到源可用性。
        let mirror = base.join("mirror");
        std::fs::create_dir_all(&mirror).unwrap();

        let mut kv = std::collections::BTreeMap::new();
        kv.insert(
            "url".to_string(),
            vec![mirror.to_string_lossy().to_string()],
        );
        kv.insert(
            "dir".to_string(),
            vec![install_dir.to_string_lossy().to_string()],
        );
        let a = Args {
            cmd: "upgrade".to_string(),
            pos: Vec::new(),
            kv,
        };
        let r = cmd_upgrade(&a);

        assert!(
            install_dir.exists(),
            "--dir 指向的目录要在写临时文件之前建出来，否则 curl 会以 23 失败并被报成源不可达"
        );
        let e = r.expect_err("空镜像没有 manifest，应当失败");
        let msg = e.msg();
        assert!(
            msg.contains("本地目录") || msg.contains("本地源"),
            "要指出是本地源缺文件: {msg}"
        );
        assert!(!msg.contains("不可达"), "不能说成不可达: {msg}");

        let _ = std::fs::remove_dir_all(&base);
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
