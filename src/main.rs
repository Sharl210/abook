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
    let on_path = !found.is_empty();
    let ready = on_path;
    if a.has("json") {
        println!(
            "{{\"os\":{},\"arch\":{},\"target\":{},\"exe\":{},\"on_path\":{},\
\"ready\":{},\"found\":[{ }],\"self\":{},\"root\":{}}}",
            jstr(os),
            jstr(arch),
            jstr(target),
            jstr(exe),
            on_path,
            ready,
            found.iter().map(|f| jstr(f)).collect::<Vec<_>>().join(","),
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
        println!("PATH 自检   通过（裸名 abook 可直接调用）");
        for f in &found {
            println!("            {f}");
        }
        println!("结论        就绪");
        Ok(0)
    } else {
        println!("PATH 自检   未通过：PATH 里找不到名为 {exe} 的可执行文件");
        println!("结论        需要初始化");
        println!("下一步      abook install");
        Ok(4)
    }
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
    let base = a
        .get("url")
        .map(|s| s.to_string())
        .or_else(|| std::env::var("ABOOK_BASE_URL").ok())
        .unwrap_or_else(|| {
            "https://github.com/Sharl210/abook/releases/latest/download".to_string()
        });
    let url = format!("{base}/{art}");
    if a.has("dry-run") || a.has("json") {
        println!(
            "{{\"os\":{},\"arch\":{},\"target\":{},\"url\":{},\"exe\":{}}}",
            jstr(os),
            jstr(arch),
            jstr(target),
            jstr(&url),
            jstr(exe)
        );
        return Ok(0);
    }
    println!("平台 {os}/{arch} → {target}");
    println!("下载 {url}");
    // find a downloader
    let dl = if let Ok(o) = std::process::Command::new("curl").arg("--version").output() {
        if o.status.success() {
            Some(vec!["curl", "-fL", "-o"])
        } else {
            None
        }
    } else {
        None
    };
    let dl = match dl {
        Some(d) => d,
        None => {
            match std::process::Command::new("wget").arg("--version").output() {
                Ok(o) if o.status.success() => vec!["wget", "-O"],
                _ => {
                    return blocked(
                        "找不到 curl 或 wget，无法下载。请手动下载对应架构产物并放入 PATH。",
                    )
                }
            }
        }
    };
    let dest = home_dir().join(".local").join("bin").join(exe);
    ensure_dir(&dest.parent().map(|p| p.to_path_buf()).unwrap_or(home_dir()))?;
    let st = std::process::Command::new(dl[0])
        .args(&dl[1..])
        .arg(&dest)
        .arg(&url)
        .status();
    match st {
        Ok(s) if s.success() => {}
        Ok(s) => return EE::Io(format!("下载失败，退出码 {}", s.code().unwrap_or(-1))).into_err(),
        Err(e) => return EE::Io(format!("无法执行下载器: {e}")).into_err(),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
    }
    println!("已安装 {}", dest.display());
    println!();
    println!("把下面这行加入 shell 配置后重开终端：");
    println!("  export PATH=\"$HOME/.local/bin:$PATH\"");
    println!("验证: abook version");
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

fn dispatch(a: &Args) -> R<i32> {
    if a.has("help") || a.cmd == "help" || a.cmd == "--help" {
        print!("{}", help_text());
        return Ok(0);
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
