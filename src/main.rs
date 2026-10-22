//! abook — the AgentsBook memory-book command line tool.
//!
//! One binary, no dependencies, every memory-book operation behind a
//! subcommand. The tool owns the mechanics; the model only needs to know the
//! commands.

mod atlas;
mod book;
mod cmd_admin;
mod cmd_read;
mod cmd_vault;
mod cmd_write;
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

读
  state                     库状态：记录数、地图数、收录时刻、一致性
  stats                     分布统计：KIND / MATURITY / 分支 / 已晋升
  recall <查询词>            分级召回：词面 → READ_WHEN → 分支摘要
      --lib canonical|candidate|both   --limit N   --body   --json
      --include-promoted    把已晋升候选也纳入
  search <模式>              全文/文件名搜索，支持 * 与 ?
  show <记录>                看一条记录（路径 / 文件名 / ID 均可）
  atlas [canonical|candidate] 看地图分支
  index [关键词]             看 Index.md 路由
  recent                     最近变更记录
  paths                      打印各路径
  lint                       记录头完整性与命名规范检查
  doctor [--fix]             健康检查，--fix 自动修可修项
  reconcile                  目录 / 地图 / 名册三方对账

写（正式库需要 --approve）
  write --title T --read-when W --body-file F [--lib candidate|canonical]
      --kind --maturity --authority --summary --keys --concepts --entities
      --not-for --evidence --uncertainty --source --related --supersedes
      --branch --project --task --session --dry-run --approve
  promote <候选> [--approve] [--branch B] [--dry-run]
  update <记录> --title T ... [--approve] [--dry-run]
  delete <记录> [--approve] [--hard] [--dry-run]
  restore <记录> [--approve] [--dry-run]

保险箱
  vault list                 列条目（标识 + 用途）
  vault get <标识>           取明文（--json / --purpose-only）
  vault put <标识> --purpose P (--body B | --body-file F | --stdin)
  vault rm <标识> [--approve]
  vault ledger [--rebuild]   看或重建名册
  vault verify               逐条解密自检 + 文件名派生校验
  vault names                列出标识对应的派生文件名
  vault reconcile            文件数与名册行数对账

维护
  init                       初始化库结构
  refresh [canonical|candidate|both]   按磁盘重建地图并推进收录时刻
  normalize [canonical|candidate|both] [--dry-run]  旧命名升级为新命名方案
  commit --slug S --body-file F        追加一条变更记录
  install [--url URL]        架构自检 + 下载对应二进制 + 加入 PATH
  version                    打印版本
  help                       本帮助

环境变量
  ABOOK_ROOT        记忆库根目录（默认 ~/AgentsBook）
  ABOOK_UTC_OFFSET  UTC 偏移秒数（默认自动探测）
  ABOOK_BASE_URL    安装时下载基址

退出码
  0 成功  2 用法  3 未找到  4 需批准/受阻  5 完整性问题  6 IO
"#
    )
}

fn cmd_install(a: &Args) -> R<i32> {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    let target = match (os, arch) {
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("linux", "arm") => "armv7-unknown-linux-musleabihf",
        ("linux", "x86") => "i686-unknown-linux-musl",
        ("android", "aarch64") => "aarch64-linux-android",
        ("android", "arm") => "armv7-linux-androideabi",
        ("android", "x86_64") => "x86_64-linux-android",
        ("android", "x86") => "i686-linux-android",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("windows", "x86") => "i686-pc-windows-msvc",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        _ => return usage(format!("不支持的平台: {os}/{arch}")),
    };
    let exe = if os == "windows" { "abook.exe" } else { "abook" };
    let base = a
        .get("url")
        .map(|s| s.to_string())
        .or_else(|| std::env::var("ABOOK_BASE_URL").ok())
        .unwrap_or_else(|| {
            "https://github.com/Sharl210/abook/releases/latest/download".to_string()
        });
    let url = format!("{base}/abook-{target}");
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
        "search" => cmd_read::cmd_search(a),
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
        "install" | "self-install" => cmd_install(a),
        other => usage(format!("未知命令: {other}（运行 abook help）")),
    }
}
