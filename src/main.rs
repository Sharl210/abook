//! abook — the AgentsBook memory-book command line tool.
//!
//! One binary, no dependencies, every memory-book operation behind a
//! subcommand. The tool owns the mechanics; the model only needs to know the
//! commands.

mod atlas;
mod book;
mod parse;
mod rx;
mod search;
mod semantic;
mod paging;
mod schema;
mod selftest;
mod cmd_admin;
mod cmd_edit;
mod cmd_audit;
mod cmd_view;
mod cmd_read;
mod cmd_vault;
mod cmd_write;
mod cmd_write_sync;
mod query;
mod rec;
mod sha256;
mod util;
mod vault;

use util::*;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn help_text() -> String {
    format!(
        r#"abook {VERSION} — 记忆库命令行工具（单二进制，零依赖）

用法: abook <命令> [参数...]

═══ 先看这两条 ═══
  abook detect              我在什么平台、工具装好没有、该用哪个架构包
  abook help                本帮助
  abook selftest            内置自检（解析/校验/正则/模糊/搜索分级）

═══ 读：了解库 ═══
  state                     库状态：记录数、地图数、收录时刻、一致性
  stats                     分布：KIND / MATURITY / STATUS，支持 --page
  paths                     打印全部路径
  tree [--page]             分支 → 条目数
  budget [记录]             预载与单条读取的字节预算（对照规则上限）
  audit [--all] [--json]    全库格式审计：纠偏了多少处、有无 ERROR
  lint                      头部完整性与命名规范检查
  doctor [--fix]            健康检查
  reconcile                 目录 / 地图 / 名册三方对账

═══ 找：搜索（按代价分级，默认 L1）═══
  search <词> [更多词]      多词默认 AND
      --level N             1=字面 2=正则/模糊 3=语义 4=语义+扩展 5=深度
      --regex PAT           正则（自研引擎，支持 [] () | {{n,m}} \d \w ^ $ \b）
      --fuzzy --distance N  模糊，允许 N 处编辑
      --literal             强制字面
      --raw-lines           按 KEY=value 整行匹配（如 ^ID=）
      --semantic            语义检索（BM25 + 关联扩展）
      --deep                L5 深度扩展      --no-expand  纯词频
      --field F1,F2         限定字段        --context N  片段上下文行
      --lib canonical|candidate|both       --max N
      --include-deleted  --hide-promoted   --case  区分大小写
      --page N --per-page N  --json
  recall <查询词>            分级召回：词面 → READ_WHEN → 分支摘要
  grep <文本>               逐行原文搜索，--context N
  index [关键词]            Index.md 路由
  atlas [lib] [--card 分支] 地图分支与卡片字段
  recent                    最近变更

═══ 看：单条记录 ═══
  show <记录>               全字段 + 正文；--page/--per-page 分页；--json
                            --fields 显示完整字段值；--deviations 看纠偏
  get <记录> [字段]         取一个字段；不带字段名则列全部字段名与长度
  cat <记录>                只输出正文
  diff <记录A> <记录B>      字段级差异
  history [关键词]          变更记录，支持 --page
  export [--format jsonl|md] [--out 文件] [--lib L]   导出，支持分页

═══ 写：改动（正式库需 --approve）═══
  write --title T --read-when W --body-file F
      --kind --maturity --authority --summary --keys --concepts --entities
      --not-for --evidence --uncertainty --source --related --supersedes
      --branch --project --task --session --lib candidate|canonical
      --dry-run --approve
  set <记录> <字段> <值>    改单个字段
  unset <记录> <字段>       删单个字段
  append <记录>             追加正文（--text / --file / stdin）
  prepend <记录>            前置正文
  patch <记录> --old A --new B [--field F] [--all]   文本替换
  update <记录> --title T ...   批量改字段
  delete <记录> [--hard]    软删（默认）/ 物理删
  restore <记录>            恢复软删
  rename <记录> <新名> [--title]   改名或改标题
  move <记录> <分支>        换分支
  copy <记录> [--to canonical|candidate]   复制
  merge <记录A> <记录B> ... [--title T] [--remove-sources]   合并
  split <记录> --at <文本>  在指定位置拆成两条
  promote <候选> [--branch B]    候选 → 正式（需 --approve）
  demote <正式>             正式 → 候选（需 --approve）

═══ 修：格式问题 ═══
  check [记录|-]            检查一条（或 stdin），只报不改
  repair [记录] [--apply]   按校验器自动修复，默认预演
  normalize [lib] [--dry-run]   旧命名升级为新方案
  refresh [lib]             按磁盘重建地图并推进收录时刻
  sync                      同步 Global 计数与地图收录时刻

═══ 保险箱 ═══
  vault ls                  列条目（标识 + 用途）
  vault get <标识>          取明文；--purpose-only 只给用途
  vault put <标识> --purpose P (--body B | --body-file F | --stdin)
  vault rm <标识>           删除条目
  vault lint                名册与目录对账
  vault repair              重建名册
  vault verify              逐条解密自检
  （别名：list/ledger/names/reconcile）

═══ 安装与平台 ═══
  detect                    平台识别 + PATH 自检
  install [--url URL]       下载本机架构的二进制并放入 PATH
  version                   版本

═══════ 通用参数 ═══
  --page N --per-page N     所有列表命令都分页（默认 30，0=不分页）
  --json                    机器可读输出
  --dry-run                 预演，不写盘
  --approve                 授权写正式库
  --root PATH               指定库根（等价 ABOOK_ROOT）

═══ 环境变量 ═══
  ABOOK_ROOT        记忆库根目录（默认 ~/AgentsBook）
  ABOOK_UTC_OFFSET  UTC 偏移秒数（默认自动探测）
  ABOOK_BASE_URL    安装下载基址

═══ 退出码 ═══
  0 成功  2 用法  3 未找到  4 需批准/受阻  5 完整性问题  6 IO

═══ 每个命令接受哪些参数 ═══
写错参数名会被直接拒绝（不会静默忽略）。下面按命令列出可用的值参数；
加 --json 一般都能拿到机器可读输出。

  write      --title --read-when --not-for --body-file --lib --approve --dry-run --force
  update     --title --body --body-file --approve --dry-run
  set        --value --dry-run
  unset      --text --file --body
  append     --text --file --dry-run
  prepend    --dry-run
  patch      --old --new --field --in --all --dry-run
  rename     --title --dry-run
  move       --dry-run
  copy       --to --approve --dry-run
  merge      --title --remove-sources --approve --dry-run
  split      --at --dry-run
  delete     --hard --approve --dry-run
  restore    --approve --dry-run
  promote    --branch --approve --force --dry-run
  demote     --approve --dry-run

  show       --field --fields --page --per-page --deviations --json
  list       --lib --kind --status --maturity --tag --q --page --per-page
  recall     --q --limit --all --body --raw --include-promoted --json
  export     --lib --format --out --include-deleted
  index      --limit
  atlas      --card
  history    --limit
  recent     --limit
  budget     --json

  search     --field --lib --max --case
             --regex|--re|--literal|--exact   精确/模式匹配
             --fuzzy|--approx --distance N    模糊（编辑距离）
             --whole-line                     整行匹配
             --semantic|--sem --level 1..5    分级语义
             --deep --min-score N --context N 深度/阈值/上下文行
             --and --no-expand --lines --raw-lines --hide-promoted --include-deleted
  grep       --context

  vault      子命令：list get put del ledger export import rekey
  detect  install  version  selftest  help

═══ 参数拼写容错 ═══
  --limit / --size 会被当成 --per-page；--offset 当成 --page。
  其余拼错的名字会立刻报错并给出最接近的正确写法，不会静默当没看见。


═══ 分级搜索怎么选 ═══
  L1 2ms   确定记得原词          → search 词
  L2 20ms  记不准 / 要模式匹配    → search 词 --fuzzy / --regex
  L3 50ms  只有概念，词不确定     → search 词 --level 3
  L4 60ms  想找"换个说法的同一件事" → search 词 --semantic（默认推荐）
  L5 130ms 准确定位不到，要广撒网  → search 词 --deep
"#
    )
}

/// Architecture detection plus PATH self-check. Answers the one question the
/// calling model needs before anything else: is the tool installed and
/// reachable by bare name, and if not, which artifact does this device need?

/// The single source of truth for platform -> release artifact name.
/// `detect` and `install` both call this so they can never drift apart, and
/// the names here must match `.github/workflows/build.yml` exactly.
fn target_triple(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        ("android", "aarch64") => "aarch64-linux-android",
        ("android", "arm") => "armv7-linux-androideabi",
        ("android", "x86_64") => "x86_64-linux-android",
        ("android", "x86") => "i686-linux-android",
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("linux", "arm") => "armv7-unknown-linux-musleabihf",
        ("linux", "x86") => "i686-unknown-linux-musl",
        ("linux", "riscv64") => "riscv64gc-unknown-linux-musl",
        ("linux", "loongarch64") => "loongarch64-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-gnu",
        ("windows", "x86") => "i686-pc-windows-gnu",
        ("windows", "aarch64") => "aarch64-pc-windows-gnullvm",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        _ => return None,
    })
}

/// Release artifact file name for a target. Windows keeps the `.exe` suffix.
fn artifact_name(target: &str) -> String {
    if target.contains("windows") {
        format!("abook-{target}.exe")
    } else {
        format!("abook-{target}")
    }
}

fn cmd_detect(a: &Args) -> R<i32> {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    let target = match target_triple(os, arch) {
        Some(t) => t,
        None => return usage(format!("不支持的平台: {os}/{arch}")),
    };
    let exe = if os == "windows" { "abook.exe" } else { "abook" };
    // PATH self-check: look for a bare-named binary in every PATH entry.
    let mut found: Vec<String> = Vec::new();
    let sep = if os == "windows" { ';' } else { ':' };
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(sep) {
            if dir.is_empty() {
                continue;
            }
            let cand = std::path::Path::new(dir).join(exe);
            if cand.is_file() {
                found.push(cand.to_string_lossy().to_string());
            }
        }
    }
    // current_exe() is unreliable on Android (it can resolve to the dynamic
    // linker), so trust the PATH scan first and fall back to argv[0].
    let self_path = if let Some(first) = found.first() {
        first.clone()
    } else if let Some(a0) = std::env::args().next() {
        a0
    } else {
        String::new()
    };
    // "Found on PATH" is not the same as "installed". A build directory or a
    // throw-away workspace is on PATH only for the shell that set it up, so
    // counting it as ready reports success that evaporates next session.
    let classify = install_location_kind;
    let stable: Vec<String> = found
        .iter()
        .filter(|f| classify(f) == "stable")
        .cloned()
        .collect();
    let transient: Vec<String> = found
        .iter()
        .filter(|f| classify(f) == "transient")
        .cloned()
        .collect();
    let on_path = !found.is_empty();
    // ready means: a durable copy exists AND the bare name resolves to it.
    let ready = !stable.is_empty();
    if a.has("json") {
        println!(
            "{{\"os\":{},\"arch\":{},\"target\":{},\"exe\":{},\"on_path\":{},\
\"ready\":{},\"found\":[{ }],\"stable\":[{ }],\"transient\":[{ }],\"self\":{},\"root\":{}}}",
            jstr(os),
            jstr(arch),
            jstr(target),
            jstr(exe),
            on_path,
            ready,
            found.iter().map(|f| jstr(f)).collect::<Vec<_>>().join(","),
            stable.iter().map(|f| jstr(f)).collect::<Vec<_>>().join(","),
            transient.iter().map(|f| jstr(f)).collect::<Vec<_>>().join(","),
            jstr(&self_path),
            jstr(&resolve_root(a).to_string_lossy())
        );
        return Ok(if ready { 0 } else { 4 });
    }
    println!("平台        {os}/{arch}");
    println!("需要产物    abook-{target}{}", if exe.ends_with(".exe") { ".exe" } else { "" });
    println!("已装路径    {}", if self_path.is_empty() { "-" } else { &self_path });
    println!("记忆根      {}", resolve_root(a).display());
    if ready {
        println!("PATH 自检   通过（裸名 {exe} 可直接调用，且位置长期有效）");
        for f in &stable {
            println!("            稳定 {f}");
        }
        for f in &transient {
            println!("            临时 {f}（不在 PATH 的长期配置里，换会话可能消失）");
        }
        println!("结论        就绪");
        Ok(0)
    } else if !transient.is_empty() {
        println!("PATH 自检   只有临时副本，没有一个长期有效的位置");
        for f in &transient {
            println!("            临时 {f}");
        }
        println!("结论        需要正式安装（当前只在本次会话的 PATH 里）");
        println!("下一步      abook install");
        Ok(4)
    } else {
        println!("PATH 自检   未通过：PATH 里找不到名为 {exe} 的可执行文件");
        println!("结论        需要初始化");
        println!("下一步      abook install");
        Ok(4)
    }
}

/// Classify where a copy of the tool lives. A build directory or an ephemeral
/// workspace satisfies "on PATH" today and is gone tomorrow, so treating it as
/// installed would report readiness that does not survive the session.
pub fn install_location_kind(p: &str) -> &'static str {
    let transient = ["/target/release", "/target/debug", "/.dsh/workspaces/", "/.cargo/"];
    if transient.iter().any(|m| p.contains(m)) {
        return "transient";
    }
    if p.contains("/.local/bin/") || p.contains("/usr/bin/") || p.contains("/bin/") {
        return "stable";
    }
    "other"
}

/// Verify that downloaded bytes really are an executable for this platform.
/// An HTTP 200 is not proof: captive portals, proxies and GitHub's own 404 page
/// all return HTML with a 200/404 status. Checking magic bytes is the only
/// content-level guarantee available without a checksum.
fn looks_like_binary(bytes: &[u8], os: &str) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    if os == "windows" {
        return bytes.starts_with(b"MZ");
    }
    // ELF for linux/android, Mach-O for macOS (64/32-bit, either endianness)
    bytes.starts_with(&[0x7f, b'E', b'L', b'F'])
        || bytes.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
        || bytes.starts_with(&[0xfe, 0xed, 0xfa, 0xcf])
        || bytes.starts_with(&[0xca, 0xfe, 0xba, 0xbe])
}

/// Extract an asset id for `want` from a GitHub release JSON document.
/// The document is flat-ish (asset objects hold only scalars), so locating the
/// enclosing `{...}` for each `"name":"<want>"` is sufficient -- no full JSON
/// parser needed, which keeps the crate dependency-free.
fn asset_id_for(json: &str, want: &str) -> Option<u64> {
    // Do not hard-code the whitespace around the colon: GitHub emits
    // `"name": "..."` today but a minified or pretty-printed document may drop
    // or widen the space. Locate `"name"` then skip whatever separates it from
    // the value, so the match survives formatting differences.
    let mut from = 0usize;
    while let Some(rel) = json[from..].find("\"name\"") {
        let at = from + rel;
        let after = at + 6;
        // skip spaces, then an optional colon, then more spaces
        let rest = &json[after..];
        let trimmed = rest.trim_start();
        let trimmed = trimmed.strip_prefix(':').map(|x| x.trim_start()).unwrap_or(trimmed);
        if !trimmed.starts_with('"') {
            from = after;
            continue;
        }
        let inner = &trimmed[1..];
        let Some(close_q) = inner.find('"') else { from = after; continue };
        if &inner[..close_q] != want {
            from = after;
            continue;
        }
        // walk left to the object start
        let open = json[..at].rfind('{')?;
        // walk right to its matching close
        let mut depth = 0i32;
        let mut close = None;
        for (i, c) in json[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let obj = &json[open..close? + 1];
        // `"id"` needs the same whitespace tolerance as `"name"`.
        if let Some(p) = obj.find("\"id\"") {
            let after_id = &obj[p + 4..];
            let tail = after_id.trim_start();
            let tail = tail.strip_prefix(':').map(|x| x.trim_start()).unwrap_or(tail);
            let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(v) = digits.parse::<u64>() {
                return Some(v);
            }
        }
        from = after;
    }
    None
}

fn cmd_install(a: &Args) -> R<i32> {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    let target = match target_triple(os, arch) {
        Some(t) => t,
        None => return usage(format!("不支持的平台: {os}/{arch}")),
    };
    let art = artifact_name(target);
    let exe = if os == "windows" { "abook.exe" } else { "abook" };

    // Where to look, in order. An explicit --url wins so the operator can point
    // at a mirror or a local file server; the public release URL works when the
    // repo is public; the API endpoint is the only way to reach a *private*
    // repo's asset, and it needs a token.
    let repo = std::env::var("ABOOK_REPO").unwrap_or_else(|_| "Sharl210/abook".to_string());
    let explicit = a
        .get("url")
        .map(|s| s.to_string())
        .or_else(|| std::env::var("ABOOK_BASE_URL").ok());
    let token = a
        .get("token")
        .map(|s| s.to_string())
        .or_else(|| std::env::var("GITHUB_TOKEN").ok())
        .or_else(|| std::env::var("GH_TOKEN").ok())
        .or_else(|| std::env::var("ABOOK_TOKEN").ok());

    let mut bases: Vec<String> = Vec::new();
    if let Some(b) = &explicit {
        bases.push(b.trim_end_matches('/').to_string());
    }
    bases.push(format!("https://github.com/{repo}/releases/latest/download"));

    if a.has("dry-run") || a.has("json") {
        println!(
            "{{\"os\":{},\"arch\":{},\"target\":{},\"artifact\":{},\"exe\":{},\"bases\":[{}],\"has_token\":{}}}",
            jstr(os),
            jstr(arch),
            jstr(target),
            jstr(&art),
            jstr(exe),
            bases.iter().map(|b| jstr(b)).collect::<Vec<_>>().join(","),
            token.is_some()
        );
        return Ok(0);
    }

    println!("平台 {os}/{arch} → {target}");
    let dest = home_dir().join(".local").join("bin").join(exe);
    let dir = dest.parent().map(|p| p.to_path_buf()).unwrap_or_else(home_dir);
    ensure_dir(&dir)?;
    let part = dir.join(format!("{exe}.part"));

    let have_curl = std::process::Command::new("curl")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !have_curl {
        return blocked("本机没有 curl，无法下载。请手动下载对应架构产物并放入 PATH。");
    }

    // Fetch into `part`, then inspect the bytes before trusting the result.
    let fetch = |url: &str, headers: &[(&str, &str)]| -> bool {
        let mut c = std::process::Command::new("curl");
        c.args(["-fsSL", "--retry", "3", "--retry-delay", "2", "-o"])
            .arg(&part)
            .arg(url);
        for (k, v) in headers {
            c.arg("-H").arg(format!("{k}: {v}"));
        }
        c.status().map(|s| s.success()).unwrap_or(false)
    };

    // GitHub wants the scheme prefix, not a bare token. Sending the raw value
    // makes every authenticated request 404 -- which looks exactly like "the
    // asset does not exist" and sent me chasing the wrong thing once.
    let bearer = token.as_ref().map(|t| format!("Bearer {}", t.trim()));

    let mut ok = false;
    let mut used = String::new();

    // Attempt 1..n: plain base URLs.
    for base in &bases {
        let url = format!("{base}/{art}");
        println!("尝试 {url}");
        // For the public github.com endpoint a token in the header is fine too,
        // but it does not authorise private assets, hence the API fallback.
        let mut hdrs: Vec<(&str, &str)> = Vec::new();
        if let Some(b) = &bearer {
            hdrs.push(("Authorization", b.as_str()));
        }
        if fetch(&url, &hdrs) {
            if let Ok(b) = std::fs::read(&part) {
                if looks_like_binary(&b, os) {
                    ok = true;
                    used = url;
                    break;
                }
                println!("  返回的不是可执行文件（{} 字节），跳过", b.len());
            }
        } else {
            println!("  不可用");
        }
    }

    // Attempt last: GitHub API asset endpoint -- the only path that works for a
    // private repository, because github.com/releases/download does not honour
    // an Authorization header.
    if !ok {
        if let Some(b) = &bearer {
            let t = b;
            println!("尝试 GitHub API（私有仓库路径）");
            let api = format!("https://api.github.com/repos/{repo}/releases/latest");
            let meta = std::process::Command::new("curl")
                .args(["-fsSL", "-H"])
                .arg(format!("Authorization: {t}"))
                .arg("-H")
                .arg("Accept: application/vnd.github+json")
                .arg(&api)
                .output();
            match meta {
                Ok(o) if o.status.success() => {
                    let txt = String::from_utf8_lossy(&o.stdout).to_string();
                    match asset_id_for(&txt, &art) {
                        Some(id) => {
                            let url = format!(
                                "https://api.github.com/repos/{repo}/releases/assets/{id}"
                            );
                            println!("  asset {id}");
                            if fetch(&url, &[("Authorization", t.as_str()), ("Accept", "application/octet-stream")]) {
                                if let Ok(b) = std::fs::read(&part) {
                                    if looks_like_binary(&b, os) {
                                        ok = true;
                                        used = url;
                                    }
                                }
                            }
                        }
                        None => println!("  该 release 里没有 {art}"),
                    }
                }
                _ => println!("  查询 release 失败（令牌无效或无权访问？）"),
            }
        } else {
            println!("未提供令牌，跳过私有仓库路径（--token 或 GITHUB_TOKEN）");
        }
    }

    if !ok {
        let _ = std::fs::remove_file(&part);
        return blocked(format!(
            "所有下载路径都失败。\n  目标产物: {art}\n  试过: {}\n可行做法:\n  1) 若是私有仓库，设置 GITHUB_TOKEN 后重试\n  2) 用 --url 指向你自己的镜像或本地目录\n  3) 手动下载 {art} 放入 {}",
            bases.join("、"),
            dir.display()
        ));
    }

    // Atomic-ish publish: only replace the live binary once bytes verified.
    if dest.exists() {
        let _ = std::fs::remove_file(&dest);
    }
    std::fs::rename(&part, &dest).map_err(|e| EE::Io(format!("无法写入 {}: {e}", dest.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
    }
    println!("已安装 {} （来源 {used}）", dest.display());

    // Self-check with the freshly installed binary rather than assuming success.
    // A release can be stale: an older build sits there with fewer commands and
    // nothing in the download path notices. Running the installed binary's own
    // selftest surfaces that immediately instead of days later.
    match std::process::Command::new(&dest).arg("version").output() {
        Ok(o) if o.status.success() => {
            let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
            println!("版本: {v}");
        }
        _ => {
            println!("警告: 装好的文件跑不起来，可能架构不匹配");
            return Ok(5);
        }
    }
    match std::process::Command::new(&dest).arg("selftest").output() {
        Ok(o) if o.status.success() => {
            let last = String::from_utf8_lossy(&o.stdout)
                .lines()
                .last()
                .unwrap_or("")
                .trim()
                .to_string();
            println!("自检: {last}");
        }
        Ok(_) => println!("警告: 装好的二进制自检未通过，release 可能有问题"),
        Err(_) => println!("警告: 装好的二进制没有 selftest，release 可能是旧版本"),
    }

    let on_path = std::env::var("PATH")
        .map(|p| {
            p.split(':')
                .any(|d| std::path::Path::new(d).join(exe).exists())
        })
        .unwrap_or(false);
    if !on_path {
        println!();
        println!("把下面这行加入 shell 配置后重开终端：");
        println!("  export PATH=\"$HOME/.local/bin:$PATH\"");
    }
    Ok(0)
}

/// Tiny helper so `return EE::Io(..).into_err()` reads naturally.
trait IntoErr<T> {
    fn into_err(self) -> R<T>;
}
impl<T> IntoErr<T> for EE {
    fn into_err(self) -> R<T> {
        Err(self)
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = match Args::parse(&argv) {
        Ok(a) => match dispatch(&a) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{}: {}", e.tag(), e.msg());
                e.code()
            }
        },
        Err(e) => {
            eprintln!("{}: {}", e.tag(), e.msg());
            e.code()
        }
    };
    std::process::exit(code);
}


/// Levenshtein distance, used only to suggest a corrected parameter name.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn dispatch(a: &Args) -> R<i32> {
    if a.has("help") || a.cmd == "help" || a.cmd == "--help" {
        print!("{}", help_text());
        return Ok(0);
    }
    // Reject unknown parameters up front. Ignoring them was actively harmful:
    // a mistyped `--limt 5` returned the entire library and still looked fine.
    // `--help` is exempt so per-command help keeps working.
    let mut unknown: Vec<String> = a
        .kv
        .keys()
        .filter(|k| !Args::known(k))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        unknown.sort();
        let near = |bad: &str| -> Option<String> {
            let mut best: Option<(usize, &str)> = None;
            for k in Args::all_known() {
                let d = edit_distance(bad, k);
                if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                    best = Some((d, k));
                }
            }
            best.filter(|(d, _)| *d <= 2).map(|(_, k)| k.to_string())
        };
        let mut msg = format!("无法识别的参数: {}", unknown.join("、"));
        for u in &unknown {
            if let Some(n) = near(u) {
                msg.push_str(&format!("\n  --{u} 是否想写 --{n}？"));
            }
        }
        msg.push_str("\n运行 `abook help` 查看全部命令与参数。");
        return usage(msg);
    }
    match a.cmd.as_str() {
        "version" | "-V" | "--version" => {
            println!("abook {VERSION} ({}/{})", std::env::consts::OS, std::env::consts::ARCH);
            Ok(0)
        }
        "state" => cmd_read::cmd_state(a),
        "stats" => cmd_read::cmd_stats(a),
        "recall" | "find" => cmd_read::cmd_recall(a),
        "index-search" => cmd_read::cmd_search(a),
        "show" | "cat" => cmd_read::cmd_show(a),
        "atlas" | "map" => cmd_read::cmd_atlas(a),
        "index" => cmd_read::cmd_index(a),
        "recent" | "log" => cmd_read::cmd_recent(a),
        "paths" => cmd_read::cmd_paths(a),
        "lint" => cmd_read::cmd_lint(a),
        "doctor" | "check" => cmd_read::cmd_doctor(a),
        "reconcile" => cmd_read::cmd_reconcile(a),
        "write" | "add" => cmd_write::cmd_write(a),
        "promote" => cmd_write::cmd_promote(a),
        "update" | "edit" => cmd_write::cmd_update(a),
        "delete" | "rm" => cmd_write::cmd_delete(a),
        "restore" => cmd_write::cmd_restore(a),
        "vault" => cmd_vault::run(a),
        "init" => {
            let root = resolve_root(a);
            let made = cmd_admin::init_book(&root)?;
            if made.is_empty() {
                println!("已存在，无需初始化: {}", root.display());
            } else {
                println!("初始化 {}: {}", root.display(), made.join(", "));
            }
            Ok(0)
        }
        "refresh" | "sync" => {
            let root = resolve_root(a);
            let which = a.pos_or(0, "both");
            let libs = match which.as_str() {
                "canonical" | "c" => vec![rec::Lib::Canonical],
                "candidate" | "n" => vec![rec::Lib::Candidate],
                _ => vec![rec::Lib::Canonical, rec::Lib::Candidate],
            };
            for lib in libs {
                let n = cmd_admin::refresh_atlas(&root, lib, a.has("dry-run"))?;
                println!("{}: {n} 条路径", lib.atlas());
            }
            Ok(0)
        }
        "normalize" => {
            let root = resolve_root(a);
            let which = a.pos_or(0, "both");
            let libs = match which.as_str() {
                "canonical" | "c" => vec![rec::Lib::Canonical],
                "candidate" | "n" => vec![rec::Lib::Candidate],
                _ => vec![rec::Lib::Canonical, rec::Lib::Candidate],
            };
            let mut total = 0;
            for lib in libs {
                let ch = cmd_admin::normalize_names(&root, lib, a.has("dry-run"))?;
                for (o, n) in &ch {
                    println!("{o} -> {n}");
                }
                total += ch.len();
            }
            println!("{} 条{}", total, if a.has("dry-run") { "（预演）" } else { "已改名" });
            Ok(0)
        }
        "commit" => {
            let root = resolve_root(a);
            let slug = a.get("slug").unwrap_or("maintenance").to_string();
            let body = if let Some(f) = a.get("body-file") {
                read_str(&expand_tilde(f))?
            } else {
                a.get("body").unwrap_or("").to_string()
            };
            if body.trim().is_empty() {
                return usage("commit 需要 --body 或 --body-file");
            }
            if a.has("dry-run") {
                println!("[dry-run] 将写入变更记录: {slug}");
                return Ok(0);
            }
            let p = cmd_admin::write_commit(&root, &slug, &body)?;
            println!("{}", p.display());
            Ok(0)
        }
        "audit" => cmd_audit::cmd_audit(a),
        "check" => cmd_audit::cmd_check(a),
        "repair" | "fix" => cmd_audit::cmd_repair(a),
        "get" => cmd_view::cmd_get(a),
        "list" | "ls" => cmd_view::cmd_list(a),
        "history" | "log" => cmd_view::cmd_history(a),
        "diff" => cmd_view::cmd_diff(a),
        "export" => cmd_view::cmd_export(a),
        "tree" => cmd_view::cmd_tree(a),
        "search" | "find" => cmd_view::cmd_search(a),
        "grep" => cmd_view::cmd_grep(a),
        "budget" => cmd_view::cmd_budget(a),
        "set" => cmd_edit::cmd_set(a),
        "unset" => cmd_edit::cmd_unset(a),
        "append" => cmd_edit::cmd_append(a),
        "prepend" => cmd_edit::cmd_prepend(a),
        "patch" => cmd_edit::cmd_patch(a),
        "rename" => cmd_edit::cmd_rename(a),
        "move" => cmd_edit::cmd_move(a),
        "copy" | "cp" => cmd_edit::cmd_copy(a),
        "demote" => cmd_edit::cmd_demote(a),
        "merge" => cmd_edit::cmd_merge(a),
        "split" => cmd_edit::cmd_split(a),
        "selftest" => selftest::cmd_selftest(a),
        "detect" | "where" => cmd_detect(a),
        "install" | "self-install" => cmd_install(a),
        other => usage(format!("未知命令: {other}（运行 abook help）")),
    }
}
