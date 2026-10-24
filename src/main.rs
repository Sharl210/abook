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
mod cmd_branch;
mod spec;
mod spec_text;
mod cmd_spec;
mod cmd_edit;
mod cmd_audit;
mod attach;
mod cmd_path;
mod cmd_view;
mod cmd_read;
mod cmd_skill;
mod cmd_vault;
mod cmd_write;
mod cmd_write_sync;
mod query;
mod rec;
mod splice;
mod sha256;
mod util;
mod vault;
mod out;

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
  init [--root PATH]        初始化一个空记忆库（含地图、名册、保险箱）
  state                     库状态：记录数、地图数、收录时刻、一致性
  stats                     分布：KIND / MATURITY / STATUS，支持 --page
  paths                     打印全部路径
  tree [--flat]             分支树：条目数 / 体积 / 行数 / 最近活跃（有 PARENT 则缩进）
  path <记录|分支>          反查：记录所属分支链；给分支名则向下展开子树
  attach <子命令> <记录>    附件（文件域）：list / add / get / rm / path / check
  budget [记录]             预载与单条读取的字节预算（对照规则上限）
  audit [--all] [--json]    全库格式审计：纠偏了多少处、有无 ERROR
  lint                      头部完整性与命名规范检查
  doctor [--fix]            健康检查
  fix | repair [记录]       修正头部可自动修的项（默认预演，--apply 才写）
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
  identity [关键词]         凭据身份索引（Global.md 该节，不含任何凭据值）
  atlas [lib] [--card 分支] 地图分支与卡片字段
  recent                    最近变更
  log [--page N]            变更记录，带大小
  map [canonical|candidate] 地图概览：分支数、路径数、收录时刻

═══ 看：单条记录 ═══
  show <记录>               全字段 + 正文；--page/--per-page 分页；--json
                            --fields 显示完整字段值；--deviations 看纠偏
  get <记录> [字段]         取一个字段；不带字段名则列全部字段名与长度
  cat <记录>                只输出正文
  diff <记录A> <记录B>      字段级差异
  history [关键词]          变更记录，支持 --page
  staging [关键词]          待决暂存件；--show <名> 看全文
  review [关键词]           晋升记录；--show <名> 看全文
  export [--format jsonl|md] [--out 文件] [--lib L]   导出，支持分页

═══ 写：改动（正式库需 --approve）═══
  write --title T --read-when W --body-file F
      --kind --maturity --authority --summary --keys --concepts --entities
      --not-for --evidence --uncertainty --source --related --supersedes
      --branch --project --task --session --lib candidate|canonical
      --dry-run --approve
  edit <记录> --body-file F 整篇重写正文（--dry-run --approve）
  set <记录> <字段> <值>    改单个字段
  unset <记录> <字段>       删单个字段
  append <记录>             追加正文（--text / --file / stdin）
  prepend <记录>            前置正文
  patch <记录> --old A --new B [--field F] [--all]   文本替换
  update <记录> --title T ...   批量改字段
  delete <记录> [--hard]    软删（默认）/ 物理删
  commit --slug S --body-file F  写一条 canonical/commits 变更记录（--dry-run）
  restore <记录>            恢复软删
  rename <记录> <新名> [--title]   改名或改标题
  move <记录> <分支>        换分支
  copy <记录> [--to canonical|candidate]   复制
  merge <记录A> <记录B> ... [--title T] [--remove-sources]   合并成新记录（源记录标记 SUPERSEDED，仍可检索）
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
  commands                  机器可读：本二进制真正分派的全部命令名（每行一条）
  describe [命令]           查询某个命令允许的参数（AI 调用前先用它核对，别猜）
  branch <列出|加|改名|删> 管理地图分支卡
  recount                   按磁盘实际重算各分支 COUNT/MAXSIZE
  leftover [--apply]        扫描中断写入残留的 .tmp 文件（--apply 才删）
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
写错参数名会被直接拒绝（不会静默忽略）。下面按命令列出可用的值参数。

有 --json 的命令（输出单个可解析 JSON 文档，其余命令传了会被拒绝）：
  state stats paths tree audit list show get diff recall search grep
  index atlas recent log history budget export reconcile vault
  detect version selftest

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
             合并结果是一条新记录；--remove-sources 把源记录标成 SUPERSEDED
             （仍留在库里可检索，不等于删除）
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

  vault      子命令：ls get put rm ledger export import verify rekey
                     lint（名册对账） repair（重建名册） names（只列文件名）
  skill      子命令：guide new candidate candidates read decide lint path
                     Skill 约定与候选反哺；候选落 ~/AgentSkills/<名>/candidates/
  spec       不带名字列目录；spec <名> 取回该规范全文
                     可用：plan ulw security compaction maintenance magic skill
  detect  install  version  selftest  help

═══ 参数拼写容错 ═══
  --limit / --size 会被当成 --per-page；--offset 当成 --page。
  其余拼错的名字会立刻报错并给出最接近的正确写法，不会静默当没看见。


═══ 分级搜索怎么选 ═══
代价随库增长，下表是相对量级不是固定耗时；括号内为 508 条 / 2.7MB 实测。
  L1 最省   确定记得原词          → search 词                    （约 23ms）
  L2 中     记不准 / 要模式匹配    → search 词 --fuzzy / --regex  （约 39ms）
  L3 较高   只有概念，词不确定     → search 词 --level 3          （约 95ms）
  L4 较高   想找"换个说法的同一件事" → search 词 --semantic         （约 94ms）
  L5 最高   准确定位不到，要广撒网  → search 词 --deep              （约 98ms）

L3/L4/L5 都要先建整库索引，所以三者耗时接近；差别在扩展轮次能捞到
多少前几档漏掉的东西，而不是快慢。日常用 L1/L2，找不到再往上加。
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
        ("linux", "riscv64") => "riscv64gc-unknown-linux-gnu",
        ("linux", "loongarch64") => "loongarch64-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-gnullvm",
        ("windows", "x86") => "i686-pc-windows-gnullvm",
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
            // Ask for the tag named `latest` explicitly. `releases/latest`
            // means "most recently created release", so once a version tag is
            // published the endpoint can silently start returning that one and
            // the caller gets an older build than the one on main.
            let api = format!("https://api.github.com/repos/{repo}/releases/tags/latest");
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
        // A binary that installs but fails its own self-test is a broken
        // release, and saying so only in a warn line lets scripts (and CI) read
        // exit 0 as "installed fine". Report it the way any other failed
        // verification is reported: non-zero exit, with what to do about it.
        Ok(_) => {
            println!("警告: 装好的二进制自检未通过，release 可能有问题");
            return Ok(5);
        }
        Err(_) => {
            println!("警告: 装好的二进制没有 selftest，release 可能是旧版本");
            return Ok(5);
        }
    }

    let on_path = crate::util::path_dirs()
        .iter()
        .any(|d| std::path::Path::new(d).join(exe).exists());
    if !on_path {
        println!();
        println!("把下面这行加入 shell 配置后重开终端：");
        println!("  export PATH=\"$HOME/.local/bin:$PATH\"");
    }
    Ok(0)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let parsed = Args::parse(&argv);
    // root is resolved the same way every command resolves it, so the write
    // report can print repo-relative names
    let root = parsed
        .as_ref()
        .ok()
        .map(|a| crate::util::resolve_root(a))
        .unwrap_or_default();
    // Whether the caller asked for JSON decides how the write ledger is
    // reported: printing it after the document would corrupt the very thing
    // the flag promised.
    let want_json = parsed
        .as_ref()
        .ok()
        .map(|a| a.has("json"))
        .unwrap_or(false);
    // True only when this run's prose is being collected into a document. A
    // read command emits its own JSON and must not also receive a wrapper, or
    // it prints two documents and neither parses.
    let mut captured = false;
    let code = match parsed {
        Ok(ref a) if a.has("json") && needs_capture(&spec::canonical(&a.cmd)) => {
            crate::util::begin_capture();
            captured = true;
            match dispatch(a) {
                Ok(c) => c,
                Err(e) => {
                    // Exit here rather than falling through: the tail block
                    // would print a second, contradictory document.
                    let lines = crate::util::take_capture();
                    crate::out::report_error_lines(a, e.tag(), e.msg(), e.code(), &lines);
                    std::process::exit(e.code());
                }
            }
        }
        Ok(a) => match if a.has("help") {
            // `abook list --help` must print help even for a command whose
            // parameter set would reject `--help` itself; validating first
            // turned a documented invocation into a usage error.
            dispatch(&a)
        } else {
            a.validate_for(&spec::canonical(&a.cmd)).and_then(|_| dispatch(&a))
        } {
            Ok(c) => c,
            Err(e) => {
                crate::out::report_error(&a, e.tag(), e.msg(), e.code());
                e.code()
            }
        },
        Err(e) => {
            // Parsing failed, so there are no `Args` to inspect for `--json`.
            // Scan the raw argv instead: a caller that asked for JSON must get
            // JSON even when the request was malformed, otherwise the very
            // errors it most needs to handle arrive as prose.
            let raw_json = std::env::args().any(|x| x == "--json");
            if raw_json {
                crate::out::report_error(
                    &crate::util::Args::empty(),
                    e.tag(),
                    e.msg(),
                    e.code(),
                );
            } else {
                eprintln!("{}: {}", e.tag(), e.msg());
            }
            e.code()
        }
    };
    // `selftest` prints its own summary as the last line, and the harness that
    // reads it takes `tail -1`. A write performed by a self-test case therefore
    // must not leave a report queued: it lands *after* the summary and replaces
    // it, so the count silently became "写入 1 个文件" instead of "自检 112 通过".
    // Drain the report unconditionally for `selftest` and print nothing.
    let is_selftest = std::env::args().nth(1).as_deref() == Some("selftest");
    // The document is emitted for a non-zero exit too. `check`, `repair`,
    // `leftover` and `preflight` all exit 5 to mean "ran fine, found real
    // problems"; gating on `code == 0` dropped their whole report and returned
    // an empty stream with no explanation of why.
    if !root.as_os_str().is_empty() && !is_selftest {
        if captured {
            let names = crate::util::take_write_report(&root);
            let lines = crate::util::take_capture();
            crate::out::report_ok(&root, &names, &lines, code);
        } else if !want_json && code == 0 {
            crate::util::flush_write_report(&root);
        }
    }
    std::process::exit(code);
}


/// Levenshtein distance, used only to suggest a corrected parameter name.
pub fn edit_distance(a: &str, b: &str) -> usize {
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
    // `--help` as a flag keeps per-command help working without a second table.
    if a.has("help") {
        print!("{}", help_text());
        return Ok(0);
    }
    // Reject unknown parameters up front. Ignoring them was actively harmful:
    // a mistyped `--limt 5` returned the entire library and still looked fine.
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
    match routes().iter().find(|(n, _)| *n == a.cmd) {
        Some((_, f)) => f(a),
        None => usage(format!("未知命令: {}（运行 abook help）", a.cmd)),
    }
}

/// A command handler: takes the parsed arguments, returns an exit code.
///
/// Commands whose body is short used to be written inline inside `match`, which
/// meant the dispatch list and the advertised name list were two hand-kept
/// copies that could disagree (`leftover` sat in both while its arm was gone).
/// Every handler is a plain function now so `routes()` is the only place a name
/// exists.
type Handler = fn(&Args) -> R<i32>;

/// The single source of truth for "which names does this binary answer to".
///
/// Aliases are separate rows on purpose: an alias is a name a user can type, so
/// it has to appear in `commands`, in the help check and in the spec check.
/// Commands that report in prose and therefore capture it under `--json`.
///
/// The read commands are deliberately absent: they emit a purpose-built
/// document and must not be wrapped, or their shape changes under callers that
/// were already working. Everything else that accepts `--json` has no document
/// of its own, so its sentences become one.
pub fn needs_capture(c: &str) -> bool {
    // These already write a document of their own under `--json`. Wrapping them
    // produced two objects on stdout, which is not parseable as either.
    if matches!(c, "install" | "self-install" | "detect" | "where" | "commands" | "describe" | "selftest") {
        return false;
    }
    matches!(
        c,
        "help"
            | "write" | "add" | "update" | "edit" | "delete" | "rm" | "restore"
            | "promote" | "demote" | "set" | "unset" | "append" | "prepend"
            | "patch" | "rename" | "move" | "copy" | "cp" | "merge" | "split"
            | "repair" | "fix" | "normalize" | "recount" | "vault" | "init"
            | "commit" | "refresh" | "sync" | "leftover"
    )
}

fn routes() -> &'static [(&'static str, Handler)] {
    &[
        // -- meta
        ("help", cmd_help),
        ("--help", cmd_help),
        ("-h", cmd_help),
        ("commands", cmd_commands),
        ("version", cmd_version),
        // -- skill
        ("skill", cmd_skill::cmd_skill),
        ("spec", cmd_spec::cmd_spec),
        ("--version", cmd_version),
        ("-V", cmd_version),
        ("describe", cmd_branch::cmd_describe),
        // -- read
        ("state", cmd_read::cmd_state),
        ("stats", cmd_read::cmd_stats),
        ("recall", cmd_read::cmd_recall),
        ("index-search", cmd_read::cmd_search),
        ("show", cmd_read::cmd_show),
        ("cat", cmd_read::cmd_show),
        ("atlas", cmd_read::cmd_atlas),
        ("map", cmd_read::cmd_atlas),
        ("index", cmd_read::cmd_index),
        ("identity", cmd_read::cmd_identity),
        ("recent", cmd_read::cmd_recent),
        ("paths", cmd_read::cmd_paths),
        ("lint", cmd_read::cmd_lint),
        ("doctor", cmd_read::cmd_doctor),
        ("reconcile", cmd_read::cmd_reconcile),
        ("staging", cmd_view::cmd_staging),
        ("review", cmd_view::cmd_review),
        // -- search & views
        ("search", cmd_view::cmd_search),
        ("find", cmd_view::cmd_search),
        ("grep", cmd_view::cmd_grep),
        ("list", cmd_view::cmd_list),
        ("ls", cmd_view::cmd_list),
        ("history", cmd_view::cmd_history),
        ("log", cmd_view::cmd_history),
        ("get", cmd_view::cmd_get),
        ("diff", cmd_view::cmd_diff),
        ("tree", cmd_view::cmd_tree),
        ("path", cmd_path::cmd_path),
        ("attach", attach::cmd_attach),
        ("budget", cmd_view::cmd_budget),
        ("export", cmd_view::cmd_export),
        // -- write
        ("write", cmd_write::cmd_write),
        ("add", cmd_write::cmd_write),
        ("promote", cmd_write::cmd_promote),
        ("update", cmd_write::cmd_update),
        ("edit", cmd_write::cmd_update),
        ("delete", cmd_write::cmd_delete),
        ("rm", cmd_write::cmd_delete),
        ("restore", cmd_write::cmd_restore),
        // -- edit records in place
        ("set", cmd_edit::cmd_set),
        ("unset", cmd_edit::cmd_unset),
        ("append", cmd_edit::cmd_append),
        ("prepend", cmd_edit::cmd_prepend),
        ("patch", cmd_edit::cmd_patch),
        ("rename", cmd_edit::cmd_rename),
        ("move", cmd_edit::cmd_move),
        ("copy", cmd_edit::cmd_copy),
        ("cp", cmd_edit::cmd_copy),
        ("demote", cmd_edit::cmd_demote),
        ("merge", cmd_edit::cmd_merge),
        ("split", cmd_edit::cmd_split),
        // -- audit
        ("audit", cmd_audit::cmd_audit),
        ("check", cmd_audit::cmd_check),
        ("repair", cmd_audit::cmd_repair),
        ("fix", cmd_audit::cmd_repair),
        // -- admin
        ("init", cmd_init),
        ("refresh", cmd_refresh),
        ("sync", cmd_refresh),
        ("normalize", cmd_normalize),
        ("commit", cmd_commit),
        ("branch", cmd_branch::cmd_branch),
        ("recount", cmd_branch::cmd_recount),
        ("leftover", cmd_branch::cmd_leftover),
        ("vault", cmd_vault::run),
        // -- environment
        ("selftest", selftest::cmd_selftest),
        ("detect", cmd_detect),
        ("where", cmd_detect),
        ("install", cmd_install),
        ("self-install", cmd_install),
    ]
}

/// Every name this binary answers to, sorted and de-duplicated.
///
/// Derived from `routes()` rather than written out again, so a name can never
/// be advertised here while its dispatch arm is missing.
pub fn known_commands() -> &'static [&'static str] {
    static NAMES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let mut v: Vec<&'static str> = routes().iter().map(|(n, _)| *n).collect();
        v.sort_unstable();
        v.dedup();
        v
    })
}

fn cmd_help(_a: &Args) -> R<i32> {
    // Routed through the output layer so `--json` returns one document instead
    // of a screen of prose. A `print!` here bypassed it entirely and the flag
    // was accepted, then ignored -- the exact shape this work exists to remove.
    for line in help_text().lines() {
        crate::say_line!("{line}");
    }
    Ok(0)
}

/// Machine-readable: the authoritative list of names this binary dispatches.
/// Tools and CI read this instead of scraping help text, where a `--fix` flag
/// looks exactly like a `fix` command.
fn cmd_commands(a: &Args) -> R<i32> {
    // `-h` / `--version` are dispatchable names but not commands, and the
    // documented contract for this output is "每条命令一行". Filtering here
    // keeps that output byte-stable for the docs checker while `routes()`
    // still knows about every name a user can type.
    let rows: Vec<String> = known_commands()
        .iter()
        .filter(|n| !n.starts_with('-'))
        .map(|n| n.to_string())
        .collect();
    // Machine-readable form. `commands` is the inventory every script starts
    // from: without this branch a caller parsing `--json` got the usage error
    // text and an empty stream, and the docs checker read four real commands as
    // nonexistent because it asked for JSON.
    if a.has("json") {
        let items: Vec<String> = rows
            .iter()
            .map(|n| format!("{{\"command\":{}}}", jstr(n)))
            .collect();
        let total = items.len();
        let (shown, pg) = crate::paging::page_json(a, items);
        let body = shown.join(",");
        println!(
            "{{\"count\":{},\"total\":{total},{pg},\"commands\":[{body}]}}",
            shown.len()
        );
        return Ok(0);
    }
    let (shown, footer) = crate::paging::page_rows(a, rows);
    for r in &shown {
        println!("{r}");
    }
    if !footer.is_empty() {
        println!("{footer}");
    }
    Ok(0)
}

/// Fingerprint of the sources this binary was compiled from, injected by
/// `build.rs`. Reported by `version` so a stale binary can be detected by
/// content instead of by mtime; see `tools/preflight.sh` step 3.
pub(crate) const SRC_FP: &str = env!("ABOOK_SRC_FP");

fn cmd_version(a: &Args) -> R<i32> {
    if a.has("json") {
        println!(
            "{{\"name\":\"abook\",\"version\":{},\"os\":{},\"arch\":{},\"src_fp\":{}}}",
            jstr(VERSION),
            jstr(std::env::consts::OS),
            jstr(std::env::consts::ARCH),
            jstr(SRC_FP)
        );
    } else {
        println!(
            "abook {VERSION} ({}/{}) src={SRC_FP}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }
    Ok(0)
}

fn cmd_init(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let made = cmd_admin::init_book(&root)?;
    if made.is_empty() {
        crate::say_line!("已存在，无需初始化: {}", root.display());
    } else {
        crate::say_line!("初始化 {}: {}", root.display(), made.join(", "));
    }
    Ok(0)
}

fn cmd_refresh(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let libs = libs_from(a);
    let mut parts: Vec<String> = Vec::new();
    for lib in libs {
        let n = cmd_admin::refresh_atlas(&root, lib, a.dry_run())?;
        if a.has("json") {
            let gone = n
                .dropped
                .iter()
                .map(|(b, p)| format!("{{\"branch\":{},\"path\":{}}}", jstr(b), jstr(p)))
                .collect::<Vec<_>>()
                .join(",");
            parts.push(format!(
                "{{\"atlas\":{},\"paths\":{},\"dropped\":[{}],\"dry_run\":{}}}",
                jstr(lib.atlas()),
                n.mapped,
                gone,
                a.dry_run()
            ));
        } else {
            crate::say_line!("{}: {} 条路径", lib.atlas(), n.mapped);
            report_dropped(&n.dropped);
        }
    }
    if a.has("json") {
        crate::say_line!("{{\"libs\":[{}]}}", parts.join(","));
    }
    Ok(0)
}

/// Say what a rebuild removed, item by item.
///
/// Dropping a `PATH=` entry is a claim that a record no longer exists. Doing it
/// silently is how a whole branch can go from 15 records to 0 with the only
/// visible signal being a smaller number in a summary line. Silent removal also
/// erases the evidence needed to notice the cause -- here, a rename that left
/// the map pointing at old filenames.
fn report_dropped(dropped: &[(String, String)]) {
    if dropped.is_empty() {
        return;
    }
    // A handful of dangling entries is ordinary: someone deleted a record by
    // hand. A flood is not -- it means the map and the directory disagree about
    // *every* name, which is a rename or a wrong root, not a deletion. Say so
    // differently, because the fix is different.
    let flood = dropped.len() >= 10;
    if flood {
        crate::say_line!(
            "  ⚠ {} 条路径在磁盘上找不到对应文件。这个数量不像是手工删除，\
更像是地图与目录整体脱节（改名后未同步，或 --root 指错了库）。",
            dropped.len()
        );
        crate::say_line!("    这些条目已从地图移除；如果确属改名，用 `abook normalize` 会把地图一起改。");
    } else {
        crate::say_line!("  移除 {} 条指向不存在文件的路径:", dropped.len());
    }
    let show = if flood { 5 } else { dropped.len() };
    for (branch, path) in dropped.iter().take(show) {
        crate::say_line!("    {branch}: {path}");
    }
    if dropped.len() > show {
        crate::say_line!("    …还有 {} 条", dropped.len() - show);
    }
}

fn cmd_normalize(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let libs = libs_from(a);
    let mut total = 0;
    for lib in libs {
        let ch = cmd_admin::normalize_names(&root, lib, a.dry_run())?;
        for (o, n) in &ch {
            crate::say_line!("{o} -> {n}");
        }
        total += ch.len();
    }
    crate::say_line!("{} 条{}", total, if a.dry_run() { "（预演，加 --apply 才写盘）" } else { "已改名" });
    Ok(0)
}

fn cmd_commit(a: &Args) -> R<i32> {
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
        crate::say_line!("[dry-run] 将写入变更记录: {slug}");
        return Ok(0);
    }
    let op = a.get("operation").unwrap_or("PATCH").to_uppercase();
    let library = a.get("library").unwrap_or("NONE").to_uppercase();
    let summary = a.get("summary").unwrap_or(&slug);
    let target = a.get("target").unwrap_or("-");
    let p = cmd_admin::write_commit(
        &root,
        &cmd_admin::CommitInfo {
            slug: &slug,
            operation: &op,
            library: &library,
            summary: &summary,
            target: &target,
            scope: a.get("scope").unwrap_or("按 --target 指定范围"),
            body: &body,
        },
    )?;
    if a.has("json") {
        crate::say_line!(
            "{{\"path\":{},\"slug\":{},\"bytes\":{}}}",
            jstr(&rel(&root, &p)),
            jstr(&slug),
            body.len()
        );
    } else {
        crate::say_line!("{}", p.display());
    }
    Ok(0)
}

/// Public wrapper so `describe` and the parser share one table.
pub fn command_spec(c: &str) -> Option<(String, String, String)> {
    spec::spec_for(c)
}
pub fn command_takes_json(c: &str) -> bool {
    spec::takes_json(c)
}
