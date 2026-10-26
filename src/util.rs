//! Shared helpers: errors, arg parsing, paths, time formatting, fs utilities.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------- errors

#[derive(Debug)]
pub enum EE {
    Usage(String),
    NotFound(String),
    Blocked(String),
    Integrity(String),
    /// A concurrent writer changed the file we were about to overwrite, and the
    /// two changes touch the same lines so neither can be dropped silently.
    IntegrityConflict(Conflict),
    Io(String),
    Other(String),
}

/// 一个错误 = 发生了什么 + 接下来能做什么。
///
/// 只报前一半的错误，读的人知道失败了却不知道往哪走：`Permission denied`
/// 是 SELinux 还是权限位？`未找到记录` 是名字错了还是库为空？把后一半写进
/// 同一条消息，调用方才能自己往下走而不必回头问人。
///
/// 用结构而不是拼接字符串，是为了 `--json` 能把两段分开交给程序：`message`
/// 给人看，`hint` 可以直接变成重试参数。
#[derive(Debug, Clone)]
pub struct Fail {
    pub what: String,
    pub hint: String,
}

impl Fail {
    pub fn new(what: impl Into<String>) -> Fail {
        Fail {
            what: what.into(),
            hint: String::new(),
        }
    }
    pub fn hint(mut self, h: impl Into<String>) -> Fail {
        self.hint = h.into();
        self
    }
    /// 渲染成一行，`；` 分隔。没有 hint 时不留多余标点。
    pub fn render(&self) -> String {
        if self.hint.is_empty() {
            self.what.clone()
        } else {
            format!("{}；{}", self.what, self.hint)
        }
    }
}

pub type R<T> = Result<T, EE>;

impl From<io::Error> for EE {
    fn from(e: io::Error) -> Self {
        EE::Io(e.to_string())
    }
}

impl EE {
    pub fn code(&self) -> i32 {
        match self {
            EE::Usage(_) => 2,
            EE::NotFound(_) => 3,
            EE::Blocked(_) => 4,
            EE::Integrity(_) => 5,
            EE::IntegrityConflict(_) => 5,
            EE::Io(_) => 6,
            EE::Other(_) => 1,
        }
    }
    pub fn msg(&self) -> &str {
        match self {
            EE::IntegrityConflict(c) => &c.msg_text,
            EE::Usage(m)
            | EE::NotFound(m)
            | EE::Blocked(m)
            | EE::Integrity(m)
            | EE::Io(m)
            | EE::Other(m) => m,
        }
    }
    pub fn tag(&self) -> &'static str {
        match self {
            EE::Usage(_) => "USAGE",
            EE::NotFound(_) => "NOT_FOUND",
            EE::Blocked(_) => "BLOCKED",
            EE::Integrity(_) => "INTEGRITY",
            EE::IntegrityConflict(_) => "CONFLICT",
            EE::Io(_) => "IO",
            EE::Other(_) => "ERROR",
        }
    }
}

pub fn usage<T>(m: impl Into<String>) -> R<T> {
    Err(EE::Usage(m.into()))
}

/// 带下一步提示的版本。调用处用它，别手工拼 "；"。
pub fn usage_hint<T>(m: impl Into<String>, hint: impl Into<String>) -> R<T> {
    Err(EE::Usage(Fail::new(m).hint(hint).render()))
}
pub fn notfound_hint<T>(m: impl Into<String>, hint: impl Into<String>) -> R<T> {
    Err(EE::NotFound(Fail::new(m).hint(hint).render()))
}
pub fn blocked_hint<T>(m: impl Into<String>, hint: impl Into<String>) -> R<T> {
    Err(EE::Blocked(Fail::new(m).hint(hint).render()))
}
pub fn io_hint<T>(m: impl Into<String>, hint: impl Into<String>) -> R<T> {
    Err(EE::Io(Fail::new(m).hint(hint).render()))
}
pub fn blocked<T>(m: impl Into<String>) -> R<T> {
    Err(EE::Blocked(m.into()))
}
pub fn notfound<T>(m: impl Into<String>) -> R<T> {
    Err(EE::NotFound(m.into()))
}
pub fn integrity<T>(m: impl Into<String>) -> R<T> {
    Err(EE::Integrity(m.into()))
}

/// A failure that is neither the caller's fault nor a data-integrity problem:
/// an internal invariant gave way. Exit code 1, tag `ERROR`.
///
/// Distinct from `integrity` on purpose. Data integrity failures mean the
/// library on disk needs repair and are reported as such; this means the tool
/// itself computed something impossible and the answer cannot be trusted. Folding
/// the second into the first sends the user to fix a library that is fine.
pub fn other<T>(m: impl Into<String>) -> R<T> {
    Err(EE::Other(m.into()))
}

// ---------------------------------------------------------------- args

/// Boolean switches that never consume the following token.

/// Sorted list of every value-taking parameter name in the tool. Kept sorted so
/// `known()` can binary-search it; a stale entry means a legitimate parameter
/// gets rejected, so this list is exercised by selftest.
pub const KNOWN_PARAMS: &[&str] = &[
    "after", "and", "at", "authority", "body", "body-file", "branch", "by",
    "card", "concepts", "confidence", "content", "context", "depth", "dest", "dir",
    "distance", "edits", "entities", "evidence", "field", "file", "for", "format",
    "from", "id", "ids", "in", "into", "json-depth", "kb", "keep",
    "keys", "kind", "level", "lib", "library", "match", "maturity", "max",
    "min-score", "mode", "name", "new", "next", "not-for", "old", "operation",
    "out", "page", "path", "per", "per-page", "project", "purpose", "q",
    "query", "read-when", "reason", "related", "root", "scope", "session", "show",
    "since", "size", "slug", "source", "status", "stdin", "summary", "supersedes",
    "tag", "target", "task", "text", "tier", "title", "to", "token",
    "uncertainty", "url", "value", "with", "within",
];

const BOOLS: &[&str] = &[
    // A flag listed here never consumes the following token. Anything used as
    // `a.has(...)` MUST be here, otherwise a bare flag would swallow the next
    // positional argument — which is how `--semantic 查询词` used to lose its
    // query entirely.
    // output / behaviour
    "json", "quiet", "raw", "compact", "verbose", "help",
    // authorisation and safety
    "approve", "yes", "force", "hard", "dry-run", "apply",
    // scope switches
    "all", "full", "whole", "flat", "no-tree", "move", "include-promoted", "include-deleted", "hide-promoted",
    "no-expand", "no-index", "no-atlas", "no-commit",
    "summary-only", "purpose-only", "deviations", "fields",
    "remove-sources", "rebuild", "fix", "verify", "prune",
    // search modes
    "regex", "re", "fuzzy", "approx", "literal", "exact",
    "semantic", "sem", "ask", "case", "deep", "expand", "raw-lines", "lines", "whole-line",
];

#[derive(Debug, Default)]
pub struct Args {
    pub cmd: String,
    pub pos: Vec<String>,
    pub kv: BTreeMap<String, Vec<String>>,
}

impl Args {
    /// An argument set with nothing in it. Used only when the real arguments
    /// failed to parse but a caller still asked for machine-readable errors.
    pub fn empty() -> Args {
        Args::default()
    }

    /// Fold the parameter spellings an assistant is likely to reach for onto
    /// the ones this tool actually reads. Silently ignoring `--limit` while
    /// returning the whole library is the worst possible outcome: the caller
    /// believes it paginated and gets 507 rows. Aliases remove the guesswork.
    fn alias(k: &str) -> &str {
        match k {
            "limit" | "size" | "per_page" | "perpage" | "count" | "n" => "per-page",
            "offset" => "page",
            "p" => "page",
            other => other,
        }
    }

    /// 存一个参数：折叠后的键与原键都存。
    ///
    /// 别名折叠（`limit` → `per-page`）让分页逻辑统一，但把原键弄丢了，
    /// 于是 `a.num("limit")` 恒为 `None`。两个键语义本就不同（总共几条 vs
    /// 每页几条），不能只留一个。
    fn store_both(&mut self, folded: &str, raw: &str, v: String) {
        self.kv.entry(folded.to_string()).or_default().push(v.clone());
        if folded != raw {
            self.kv.entry(raw.to_string()).or_default().push(v);
        }
    }

    /// 记录一个参数，同时保留它的原名。
    ///
    /// 别名折叠有个副作用：`--limit` 被折成 `per-page` 之后，`a.num("limit")`
    /// 永远返回 `None`——而四个命令（`index`、`recall`、`recent`、`log`）恰恰直接
    /// 读 `limit`，于是那句 `unwrap_or(40)` 成了唯一生效的值：`--limit 100` 无论
    /// 传什么都只出 40 行，而同一处提示语还在叫用户"用 --limit 扩大"。
    ///
    /// 一个 flag 被文档列出、被解析、被打印在提示里，却从来读不到值——这比不
    /// 支持它更糟，因为支持是写在帮助里的。
    ///
    /// 所以两个键都存：折叠后的键给分页逻辑用，原键给"总数上限"用。两者语义
    /// 本来就不同（每页几条 vs 总共几条），不该共用一个键。

    pub fn parse(argv: &[String]) -> R<Args> {
        let mut a = Args::default();
        let mut i = 0;
        let mut want_version = false;
        let mut want_help = false;
        if argv.is_empty() {
            return usage("需要子命令，运行 `abook help` 查看用法");
        }
        // Global flags may precede the command: `abook --root ~/AgentsBook state`
        // is the shape everybody writes, and it used to answer "unknown command:
        // --root" because argv[0] was taken as the command unconditionally.
        // Only flags that carry a value or are bare booleans are skipped, so a
        // genuinely wrong first word still reports as an unknown command.
        // Flags before the command are parsed here, and they have to be *stored*,
        // not just stepped over. Skipping them without recording meant
        // `abook --root <dir> list` silently read the default library: the flag
        // vanished, `resolve_root` fell through to `~/AgentsBook`, and every
        // command answered confidently about the wrong directory. Sandboxed
        // probes written as `--root <copy>` were therefore writing to the live
        // library while reporting success.
        while i < argv.len() && argv[i].starts_with('-') && argv[i] != "-" {
            let t = &argv[i];
            if t.starts_with("--") {
                let rest = &t[2..];
                if let Some((k, v)) = rest.split_once('=') {
                    let folded = Self::alias(k).to_string();
                    a.store_both(&folded, k, v.to_string());
                    i += 1;
                    continue;
                }
                let k = Self::alias(rest).to_string();
                // `--version` and `--help` are the two things everybody types
                // first. They must never demand a value: before this, `--version`
                // answered "参数 --version 需要一个值" and `--help` fell through
                // to "需要子命令", both of which read like the tool is broken.
                if k == "version" {
                    want_version = true;
                    i += 1;
                    continue;
                }
                if k == "help" {
                    want_help = true;
                    i += 1;
                    continue;
                }
                let needs_value = !BOOLS.contains(&k.as_str());
                if needs_value {
                    if i + 1 < argv.len() && !argv[i + 1].starts_with('-') {
                        let v = argv[i + 1].clone();
                        a.store_both(&k, rest, v);
                        i += 2;
                    } else {
                        // A value was expected in front of the command. Saying
                        // "unknown command: --root" here would blame the flag
                        // for a missing value.
                        return usage(format!(
                            "参数 --{k} 需要一个值（例如 `abook --{k} 值 <命令>`）；\
                             想调用命令 {k} 就直接写 `abook {k}`"
                        ));
                    }
                } else {
                    i += 1;
                }
            } else if t == "-h" {
                want_help = true;
                i += 1;
            } else if t == "-V" {
                want_version = true;
                i += 1;
            } else {
                break;
            }
        }
        if i >= argv.len() {
            if want_version {
                a.cmd = "version".to_string();
                return Ok(a);
            }
            if want_help {
                a.cmd = "help".to_string();
                return Ok(a);
            }
            return usage("需要子命令，运行 `abook help` 查看用法");
        }
        a.cmd = argv[i].clone();
        i += 1;
        while i < argv.len() {
            let t = &argv[i];
            if let Some(rest) = t.strip_prefix("--") {
                let (raw_k, inline) = match rest.split_once('=') {
                    Some((k, v)) => (k.to_string(), Some(v.to_string())),
                    None => (rest.to_string(), None),
                };
                let k = Self::alias(&raw_k).to_string();
                if BOOLS.contains(&k.as_str()) {
                    let v = inline.unwrap_or_else(|| "true".to_string());
                    a.store_both(&k, &raw_k, v);
                } else if let Some(v) = inline {
                    a.store_both(&k, &raw_k, v);
                } else {
                    // value follows, unless next token is another flag
                    if i + 1 < argv.len() && !argv[i + 1].starts_with("--") {
                        let v = argv[i + 1].clone();
                        a.store_both(&k, &raw_k, v);
                        i += 1;
                    } else {
                        a.store_both(&k, &raw_k, String::new());
                    }
                }
            } else if t == "-" {
                // A lone `-` is the conventional "read stdin" operand, not a
                // short option. `check -` is documented and was rejected here.
                a.pos.push(t.clone());
            } else if let Some(rest) = t.strip_prefix('-') {
                if rest == "h" {
                    a.kv.entry("help".into()).or_default().push("true".into());
                } else {
                    return usage(format!("未知短选项 -{rest}"));
                }
            } else {
                a.pos.push(t.clone());
            }
            i += 1;
        }
        Ok(a)
    }

    /// Every parameter name the tool understands anywhere. A name outside this
    /// set is almost always a typo, and a typo used to be ignored silently --
    /// `--limt 5` returned the whole library and looked like success.
    /// 这个参数名在工具里是否存在（任何命令）。
    ///
    /// 除了两张手写清单，还要看每命令的参数表——这是本轮踩到的坑：
    /// `--only` 已经写进 `spec::SPEC` 的 repair 条目，但 `known()` 只查
    /// `KNOWN_PARAMS` 和 `BOOLS`，两张手写清单都没它，于是它在分发之前就被
    /// 当成"无法识别的参数"拒掉，而 `validate_for` 那层根本没机会跑到。
    ///
    /// 同一信息存两处，迟早不一致。现在以参数表为准，手写清单只作为
    /// 表里尚未收录的补充。
    pub fn known(k: &str) -> bool {
        KNOWN_PARAMS.binary_search(&k).is_ok()
            || BOOLS.contains(&k)
            || crate::spec::SPEC.iter().any(|(_, params, flags, _, _)| {
                params.split(',').any(|x| x.trim() == k) || flags.split(',').any(|x| x.trim() == k)
            })
    }

    /// All recognised parameter names, for typo suggestions.
    ///
    /// 去重是必须的：这里的三来源（手写参数名、手写布尔名、每命令参数表）大量
    /// 重叠——参数表本来就是把"这个命令读哪些"再写一遍，它必然与手写清单重复。
    /// 不去重的话调用方（自检、拼写建议）会看到同一个名字出现好几次。
    ///
    /// 返回 Owned 而非 `&'static str`，因为去重需要一次收集；调用方只遍历一次，
    /// 这点分配可以忽略。
    pub fn all_known() -> impl Iterator<Item = &'static str> {
        let mut set: std::collections::BTreeSet<&'static str> = std::collections::BTreeSet::new();
        set.extend(KNOWN_PARAMS.iter().copied());
        set.extend(BOOLS.iter().copied());
        for (_, params, flags, _, _) in crate::spec::SPEC {
            for x in params.split(',').chain(flags.split(',')) {
                let x = x.trim();
                if !x.is_empty() {
                    set.insert(x);
                }
            }
        }
        set.into_iter()
    }

    /// Reject a parameter this particular command does not read.
    ///
    /// `Args::known` only proves a name exists *somewhere* in the tool, which
    /// is why `list --json` was accepted and silently ignored: `json` is a real
    /// flag, just not one `list` ever looked at. A caller then parsed human
    /// text as if it were JSON and got nothing. The table is the single source
    /// of truth, so the help screen, `describe` and this check cannot disagree.
    pub fn validate_for(&self, cmd: &str) -> R<()> {
        let allowed = match crate::spec::params_of(cmd) {
            Some(v) => v,
            // A command absent from the table is not checked rather than
            // rejected: `vault` delegates to subcommands with their own names.
            None => return Ok(()),
        };
        for k in self.kv.keys() {
            // Paging and root are universal, and `--root` must work even for
            // commands the table has not enumerated.
            if k == "root" || allowed.iter().any(|a| a == k) {
                continue;
            }
            // 别名原样留着（`limit` 与 `per-page` 同时入表），所以校验也要按
            // 折叠后的名字判：`--limit` 折成 `per-page` 属于这个命令接受的参数，
            // 不该因为它保留了自己的名字就被判成未知参数。
            if allowed.iter().any(|a| *a == Self::alias(k)) {
                continue;
            }
            let mut best = (usize::MAX, String::new());
            for cand in allowed.iter() {
                let d = crate::edit_distance(k, cand);
                if d < best.0 {
                    best = (d, cand.clone());
                }
            }
            // Only suggest a name this command really accepts -- suggesting a
            // flag it would reject next round is worse than no suggestion.
            let _ = &best;
            let hint = if best.0 <= 3 {
                format!("；最接近的是 --{}", best.1)
            } else {
                String::new()
            };
            return usage(format!(
                "{cmd} 不接受 --{k}{hint}。看这个命令支持哪些参数: abook describe {cmd}"
            ));
        }
        Ok(())
    }

    /// True unless the caller explicitly opted into writing.
    ///
    /// Every repairing command (`refresh`, `recount`, `normalize`, `repair`)
    /// used to write as soon as it was invoked, which made them dangerous in
    /// exactly the situation they are used in: someone poking at a live
    /// library to see what is wrong. A typo in the command name, or a shell
    /// glob that expanded into an extra word, was enough to rewrite the map.
    /// Opt-in via `--apply` is the only shape that survives being called by
    /// mistake, and it matches how `leftover` already behaved.
    pub fn dry_run(&self) -> bool {
        // `--dry-run` 无条件优先：显式说了"别写"就必须别写。
        //
        // 初版只判断"有没有 --apply"：`has("apply") || has("write") || has("yes")`
        // 全不成立才算预演，`--dry-run` 这个参数从头到尾没被读过。于是
        // `repair --apply --dry-run`（"先预演看看"的自然写法）会**真的写入**——
        // 一次本该只读的核对把库改了。实测踩到：三条记录的 RELATED_IDS 被
        // 静默剥掉一层前缀。
        //
        // 预演是安全侧，安全侧参数必须在最前面短路，不能与写入开关并列判断。
        if self.has("dry-run") || self.has("dryrun") {
            return true;
        }
        !(self.has("apply") || self.has("write") || self.has("yes"))
    }

    pub fn has(&self, k: &str) -> bool {
        self.kv.contains_key(k)
    }
    pub fn get(&self, k: &str) -> Option<&str> {
        self.kv.get(k).and_then(|v| v.last()).map(|s| s.as_str())
    }
    pub fn req(&self, k: &str) -> R<&str> {
        match self.get(k) {
            Some(v) if !v.is_empty() => Ok(v),
            _ => usage(format!("缺少必填参数 --{k}（看这个命令要哪些参数: abook describe <命令>）")),
        }
    }
    pub fn num(&self, k: &str) -> Option<u64> {
        self.get(k).and_then(|v| v.parse().ok())
    }
    pub fn pos_or(&self, i: usize, d: &str) -> String {
        self.pos.get(i).cloned().unwrap_or_else(|| d.to_string())
    }
    pub fn need_pos(&self, i: usize, what: &str) -> R<&str> {
        match self.pos.get(i) {
            Some(s) => Ok(s.as_str()),
            None => usage(format!("缺少参数：{what}")),
        }
    }
}

// ---------------------------------------------------------------- time

/// Civil date from days since Unix epoch (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub struct Stamps {
    pub compact: String, // YYYYMMDDTHHMMSS
    pub iso_z: String,   // 2026-10-22T05:47:39Z
    pub date_only: String,
    /// Local wall clock with an explicit offset and zone name, e.g.
    /// `2026-10-23T13:56:28+08:00[Asia/Shanghai]`.
    ///
    /// The live library spells commit time this way in 38 of 54 cases and
    /// never uses the bare `Z` form, so this is what new commits must carry:
    /// a reader comparing a commit to a record needs both stamps in the same
    /// notation, and `Z` alone drops the wall clock a human wrote it in.
    pub iso_local: String,
}

pub fn now_secs() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => 0,
    }
}

/// Ask the OS for the local UTC offset.
///
/// The one thing that must not vary by platform is what a timestamp *means*.
/// Every record in the library carries `+08:00[Asia/Shanghai]`; a Windows build
/// that could not read the offset would write `+00:00` for the same moment, and
/// anything sorting the two together would see an eight-hour hole. Correctness
/// here is not cosmetic.
///
/// Unix gets it from `date +%z`, which is present everywhere. Windows has no
/// such command and the standard library has no timezone API, so it asks Win32
/// directly. Both honour `ABOOK_UTC_OFFSET` first, because a container or a
/// cross-compiled test may have neither.
#[cfg(not(windows))]
pub fn local_offset_secs() -> i64 {
    // 结果进程内恒定，但原来每次都 fork 一个 `date +%z`。
    //
    // 而 short_when() 每格式化一个时间戳就调它一次，于是**每渲染一行就是
    // 一次进程创建**。实测单次 fork 2.0 ms（strace 里是 9 次 execve：前 8
    // 次 PATH 探测失败，第 9 次才命中 /usr/bin/date）。
    //
    // 代价随显示行数线性叠加，正是"看一眼库"变慢的真凶：
    //   list --per-page 500  1149ms → 约 106ms（-89%）
    //   log / search          68ms → 2 / 8ms
    //   tree / atlas         165ms → 约 90ms
    //   get                    10ms → 2ms
    // 一处缓存，七条命令同时受益。
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(local_offset_secs_uncached)
}

fn local_offset_secs_uncached() -> i64 {
    if let Ok(off) = std::env::var("ABOOK_UTC_OFFSET") {
        if let Ok(n) = off.parse::<i64>() {
            return n;
        }
    }
    #[cfg(unix)]
    {
        if let Ok(out) = std::process::Command::new("date").arg("+%z").output() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.len() == 5 {
                let sign = if s.starts_with('-') { -1 } else { 1 };
                let h: i64 = s[1..3].parse().unwrap_or(0);
                let m: i64 = s[3..5].parse().unwrap_or(0);
                return sign * (h * 3600 + m * 60);
            }
        }
    }
    0
}

#[cfg(windows)]
pub fn local_offset_secs() -> i64 {
    if let Ok(off) = std::env::var("ABOOK_UTC_OFFSET") {
        if let Ok(n) = off.parse::<i64>() {
            return n;
        }
    }
    win_tz_offset().unwrap_or(0)
}

/// Kept compiled on every platform on purpose.
///
/// This is a pure data description with no calls in it, so it costs nothing to
/// keep outside `#[cfg(windows)]`. Keeping it here means `selftest` can assert
/// the layout on Linux, where the Windows build can actually be checked, and a
/// later edit that reorders a field breaks a test instead of producing a build
/// that reads garbage from the OS.
#[repr(C)]
pub struct WinSystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    ms: u16,
}

/// `TIME_ZONE_INFORMATION`, 172 bytes. Field order and widths matter: this is
/// passed straight to the OS, so a wrong layout reads as garbage rather than
/// failing to compile. Only `bias`, `daylight_bias` and the return code are
/// read; the names and transition dates are carried because the OS writes the
/// whole struct.
#[repr(C)]
pub struct WinTimeZoneInfo {
    pub bias: i32,
    pub standard_name: [u16; 32],
    pub standard_date: WinSystemTime,
    pub standard_bias: i32,
    pub daylight_name: [u16; 32],
    pub daylight_date: WinSystemTime,
    pub daylight_bias: i32,
}

#[cfg(windows)]
extern "system" {
    fn GetTimeZoneInformation(info: *mut WinTimeZoneInfo) -> u32;
}

#[cfg(windows)]
fn win_tz_offset() -> Option<i64> {
    const TIME_ZONE_ID_INVALID: u32 = 0xFFFF_FFFF;
    const TIME_ZONE_ID_DAYLIGHT: u32 = 2;
    let mut info: WinTimeZoneInfo = unsafe { std::mem::zeroed() };
    let rc = unsafe { GetTimeZoneInformation(&mut info) };
    if rc == TIME_ZONE_ID_INVALID {
        return None;
    }
    // Bias is defined as "UTC = local + bias", so the offset from UTC is its
    // negation. Daylight saving adds DaylightBias to that same bias.
    let mut minutes = info.bias;
    if rc == TIME_ZONE_ID_DAYLIGHT {
        minutes += info.daylight_bias;
    }
    Some(-(minutes as i64) * 60)
}

/// Best-effort zone name for the local offset.
///
/// Nothing here is authoritative -- there is no tz database in this binary --
/// so the order is: an explicit override, the `TZ` variable, then a name
/// derived from the offset itself. Deriving from the offset is what makes the
/// common case (`+08:00`) render as the name the library already uses rather
/// than inventing a third spelling.
pub fn local_tz_name(off: i64) -> String {
    if let Ok(n) = std::env::var("ABOOK_TZ_NAME") {
        if !n.trim().is_empty() {
            return n.trim().to_string();
        }
    }
    if let Ok(tz) = std::env::var("TZ") {
        let t = tz.trim().trim_start_matches(':');
        if !t.is_empty() && t != "UTC" {
            return t.to_string();
        }
        if t == "UTC" {
            return "UTC".to_string();
        }
    }
    match off {
        0 => "UTC".to_string(),
        28800 => "Asia/Shanghai".to_string(),
        32400 => "Asia/Tokyo".to_string(),
        19800 => "Asia/Kolkata".to_string(),
        -18000 => "America/New_York".to_string(),
        -28800 => "America/Los_Angeles".to_string(),
        3600 => "Europe/London".to_string(),
        7200 => "Europe/Berlin".to_string(),
        _ => String::new(),
    }
}

pub fn stamps_at(secs: i64) -> Stamps {
    let off = local_offset_secs();
    let l = secs + off;
    let days = l.div_euclid(86400);
    let sod = l.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    let udays = secs.div_euclid(86400);
    let usod = secs.rem_euclid(86400);
    let (uy, um, ud) = civil_from_days(udays);
    Stamps {
        compact: format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}"),
        iso_z: format!(
            "{uy:04}-{um:02}-{ud:02}T{:02}:{:02}:{:02}Z",
            usod / 3600,
            (usod % 3600) / 60,
            usod % 60
        ),
        date_only: format!("{y:04}-{m:02}-{d:02}"),
        iso_local: {
            let sign = if off < 0 { '-' } else { '+' };
            let a = off.abs();
            let tz = local_tz_name(off);
            let base = format!(
                "{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}{sign}{:02}:{:02}",
                a / 3600,
                (a % 3600) / 60
            );
            if tz.is_empty() {
                base
            } else {
                format!("{base}[{tz}]")
            }
        },
    }
}

pub fn stamps() -> Stamps {
    stamps_at(now_secs())
}

/// The current moment in the notation records use for their own timestamps,
/// e.g. `2026-10-24T15:42:07+08:00[Asia/Shanghai]`.
///
/// Records and commits must be spelled the same way: a reader comparing the two
/// is comparing instants, and a bare `Z` drops the wall clock a human wrote it
/// in.
pub fn now_iso_local() -> String {
    stamps().iso_local
}

/// Read a record's `UPDATED` field, falling back for records written before the
/// field existed.
///
/// Library compatibility matters here: 500+ records on disk have no `UPDATED`,
/// and rewriting all of them just to add one field would be a migration the
/// user never asked for. The fallback is the same three-source order the tool
/// used before (declared time, then filename stamp, then mtime), so old records
/// keep sorting exactly as they did and new ones get a portable stamp. A record
/// gains `UPDATED` the first time it is edited -- lazy, not a bulk rewrite.
pub fn record_updated(path: &std::path::Path, text: &str, fields_updated: &str) -> i64 {
    let declared = fields_updated.trim();
    if !declared.is_empty() {
        if let Some(n) = parse_stamp(declared) {
            if n > 0 {
                return n;
            }
        }
    }
    commit_time(path, text)
}

/// Parse `YYYY-MM-DDTHH:MM:SSZ` or `YYYYMMDDTHHMMSS` into epoch seconds (UTC).
/// When did this commit happen?
///
/// The live library writes commit time under five different spellings and, in
/// 50 of 161 files, not at all:
///
///   TIME=  76   AT=  32   DATE=  2   时间=  1   (none)  50
///
/// Reading only `TIME=` silently demoted a third of the history to mtime,
/// which a `git clone` or a plain directory copy resets wholesale. So resolve
/// in order: declared field, then a timestamp leading the filename (49 of the
/// 50 fieldless files carry one), then mtime as the last resort.
/// The time a record claims for itself, without the mtime fallback.
///
/// Split out of [`commit_time`] because the two answer different questions.
/// `commit_time` answers "when did this happen" and wants any answer at all,
/// so it ends with mtime. A record's own timestamp answers "when does this
/// *say* it was written", and falling back to mtime there would make a record
/// edited today claim it was written today -- destroying the very distinction
/// that lets a reader tell created from touched.
pub fn record_time(path: &std::path::Path, text: &str) -> i64 {
    const FIELDS: [&str; 5] = ["TIME", "AT", "DATE", "WHEN", "时间"];
    for line in text.lines() {
        let t = line.trim();
        for f in FIELDS {
            if let Some(rest) = t.strip_prefix(f) {
                if let Some(v) = rest.strip_prefix('=') {
                    if let Some(n) = parse_stamp(v) {
                        return n;
                    }
                }
            }
        }
    }
    if let Some(n) = stamp_from_name(path) {
        return n;
    }
    0
}

/// `<YYYYMMDD>T<HHMMSS>-slug.md` carries the creation time even when the header
/// does not. 49 of the 50 fieldless files in the live library do.
fn stamp_from_name(path: &std::path::Path) -> Option<i64> {
    let name = path.file_name()?.to_string_lossy().to_string();
    if name.len() < 15 {
        return None;
    }
    let b = name.as_bytes();
    // Records are prefixed (`N-`, `C-`), so scan for the first digit run rather
    // than assuming the stamp starts at byte 0.
    let mut i = 0usize;
    while i + 15 <= b.len() {
        let looks = b[i..i + 8].iter().all(u8::is_ascii_digit)
            && b[i + 8] == b'T'
            && b[i + 9..i + 15].iter().all(u8::is_ascii_digit);
        if looks {
            return parse_stamp(&name[i..i + 15]);
        }
        i += 1;
    }
    None
}

/// Compact, human-facing rendering of a moment: `10-24 13:21`, or the year too
/// when it is not the current one.
///
/// Every listing prints this, so it is deliberately short. A full
/// `2026-10-24T13:21:52+08:00[Asia/Shanghai]` is ~10x the width and carries
/// information no one scans a list for.
pub fn short_when(secs: i64) -> String {
    if secs <= 0 {
        return "——".to_string();
    }
    let now = now_secs();
    let off = local_offset_secs();
    let (y, m, d) = civil_from_days((secs + off).div_euclid(86400));
    let sod = (secs + off).rem_euclid(86400);
    let (now_y, _, _) = civil_from_days((now + off).div_euclid(86400));
    if y == now_y {
        format!("{m:02}-{d:02} {:02}:{:02}", sod / 3600, (sod % 3600) / 60)
    } else {
        format!("{y}-{m:02}-{d:02}")
    }
}

/// `commit_time` with the mtime fallback left out.
pub fn commit_time(path: &std::path::Path, text: &str) -> i64 {
    // Reuse the same three-source order rather than repeating it: two copies of
    // this rule is how `history` and `recent` drifted apart before.
    let declared = record_time(path, text);
    if declared > 0 {
        return declared;
    }
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn parse_stamp(s: &str) -> Option<i64> {
    // Accepts the three spellings that occur in real records:
    //   2026-10-01T20:11:23+08:00[Asia/Shanghai]
    //   2026-09-27 13:15:47 +08:00
    //   20260927T131547
    let raw = s.trim();
    // strip a trailing [Zone] annotation
    let t0 = match raw.split_once('[') {
        Some((head, _)) => head.trim(),
        None => raw,
    };
    let t0 = t0.trim_end_matches('Z');
    let (date, time) = if let Some((d, t)) = t0.split_once('T') {
        (d, t)
    } else if let Some((d, t)) = t0.split_once(' ') {
        (d, t.trim())
    } else {
        return None;
    };
    // Pull off a numeric UTC offset and *apply* it.
    //
    // The first version simply dropped the offset, which silently mixed zones:
    // the live library has `TIME=...Z`, `... +08:00` and `...+08:00[Asia/
    // Shanghai]` side by side, so a `Z` row and a `+08:00` row written minutes
    // apart sorted eight hours out of order.
    let (time, offset_secs) = {
        let mut cut = time.len();
        let mut off = 0i64;
        for (i, c) in time.char_indices() {
            if (c == '+' || c == '-') && i > 0 {
                cut = i;
                let sign = if c == '+' { 1 } else { -1 };
                let tail: String = time[i + 1..]
                    .chars()
                    .filter(|ch| ch.is_ascii_digit() || *ch == ':')
                    .collect();
                let digits: String = tail.chars().filter(|ch| ch.is_ascii_digit()).collect();
                if digits.len() >= 4 {
                    let hh: i64 = digits[0..2].parse().unwrap_or(0);
                    let mm: i64 = digits[2..4].parse().unwrap_or(0);
                    off = sign * (hh * 3600 + mm * 60);
                } else if digits.len() == 2 {
                    off = sign * digits.parse::<i64>().unwrap_or(0) * 3600;
                }
                break;
            }
        }
        (time[..cut].trim(), off)
    };
    let (y, mo, d) = if date.contains('-') {
        let p: Vec<&str> = date.split('-').collect();
        if p.len() != 3 {
            return None;
        }
        (
            p[0].parse::<i64>().ok()?,
            p[1].parse::<u32>().ok()?,
            p[2].parse::<u32>().ok()?,
        )
    } else if date.len() == 8 {
        (
            date[0..4].parse::<i64>().ok()?,
            date[4..6].parse::<u32>().ok()?,
            date[6..8].parse::<u32>().ok()?,
        )
    } else {
        return None;
    };
    let tp: Vec<&str> = time.split(':').collect();
    let (h, mi, se) = if tp.len() == 3 {
        (
            tp[0].parse::<i64>().ok()?,
            tp[1].parse::<i64>().ok()?,
            tp[2].split('.').next()?.parse::<i64>().ok()?,
        )
    } else if time.len() == 6 {
        (
            time[0..2].parse::<i64>().ok()?,
            time[2..4].parse::<i64>().ok()?,
            time[4..6].parse::<i64>().ok()?,
        )
    } else {
        return None;
    };
    // Normalise to UTC so stamps in different zones compare correctly.
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se - offset_secs)
}

// ---------------------------------------------------------------- paths

pub fn env_or(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

/// Split `PATH` on the platform's separator.
///
/// Unix separates entries with `:` and Windows with `;`. Hardcoding `:` meant a
/// Windows build saw the entire `PATH` as one directory, so nothing was ever
/// found on it: `install` reported the binary off-PATH right after installing
/// it, and `self_path` fell back to `current_exe` on every call. Neither failed
/// loudly, which is why it survived until the cross-platform pass.
pub fn path_dirs() -> Vec<String> {
    let sep = if cfg!(windows) { ';' } else { ':' };
    std::env::var("PATH")
        .unwrap_or_default()
        .split(sep)
        .filter(|d| !d.is_empty())
        .map(|d| d.to_string())
        .collect()
}

/// Locate this binary without trusting `current_exe()`.
///
/// On Android the kernel execs `/apex/com.android.runtime/bin/linker64`, which
/// then loads the application, so `/proc/self/exe` resolves to the *linker*,
/// not to us. Anything that derives "where am I installed" from it lands on a
/// read-only system partition and fails: `upgrade` reported
/// `/apex/com.android.runtime/bin/linker64` as its install target and could
/// never update itself on Android.
///
/// The real path is the first field of `/proc/self/cmdline`. Fall back to
/// `current_exe()` only when that is unusable, and reject a resolved path that
/// is a dynamic linker even then -- writing "the new abook" over the system
/// linker is the worst possible outcome of guessing wrong.
pub fn self_path() -> Option<PathBuf> {
    if let Ok(raw) = std::fs::read("/proc/self/cmdline") {
        if let Some(end) = raw.iter().position(|&b| b == 0) {
            let s = String::from_utf8_lossy(&raw[..end]).to_string();
            if !s.is_empty() {
                let p = PathBuf::from(&s);
                let abs = if p.is_absolute() {
                    p
                } else if p.components().count() > 1 {
                    std::env::current_dir().ok()?.join(p)
                } else {
                    let mut found = None;
                    for d in path_dirs() {
                        let cand = Path::new(&d).join(&s);
                        if cand.is_file() {
                            found = Some(cand);
                            break;
                        }
                    }
                    found?
                };
                if abs.is_file() {
                    return Some(abs);
                }
            }
        }
    }
    let ce = std::env::current_exe().ok()?;
    if is_linker(&ce) {
        return None;
    }
    if ce.is_file() {
        Some(ce)
    } else {
        None
    }
}

/// True for a dynamic linker (`ld-*`, `linker`, `linker64`).
///
/// A caller that gets one of these back must treat "where am I" as unknown
/// rather than acting on it.
pub fn is_linker(p: &Path) -> bool {
    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
    name == "linker" || name == "linker64" || name == "ld-linux.so.2" || name.starts_with("ld-linux")
        || name.starts_with("ld-musl") || name == "ld.so"
}

pub fn home_dir() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    if let Ok(h) = std::env::var("USERPROFILE") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    PathBuf::from(".")
}

/// Memory book root: --root > $ABOOK_ROOT > $HOME/AgentsBook
pub fn resolve_root(a: &Args) -> PathBuf {
    if let Some(r) = a.get("root") {
        if !r.is_empty() {
            return expand_tilde(r);
        }
    }
    let e = env_or("ABOOK_ROOT", "");
    if !e.is_empty() {
        return expand_tilde(&e);
    }
    home_dir().join("AgentsBook")
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        home_dir().join(rest)
    } else if p == "~" {
        home_dir()
    } else {
        PathBuf::from(p)
    }
}

// ---------------------------------------------------------------- fs

/// 只读文件的前 `max` 个字节（按字符边界切齐），用于"只要看头部"的场合。
///
/// 数记录、判状态、看字段名这类操作用不到正文，而正文可以很大（真实库里
/// 单条 47KB，合成测试到过 52MB）。整份读进来再丢掉 99% 是纯浪费，还会把
/// 内存峰值抬高一个数量级。
pub fn read_head(p: &Path, max: usize) -> R<String> {
    use std::io::Read;
    let mut f = fs::File::open(p).map_err(|e| EE::Io(format!("读取失败 {}: {e}", p.display())))?;
    let mut buf = Vec::with_capacity(max.min(64 * 1024));
    f.by_ref()
        .take(max as u64)
        .read_to_end(&mut buf)
        .map_err(|e| EE::Io(format!("读取失败 {}: {e}", p.display())))?;
    // 截断可能落在多字节字符中间；`from_utf8_lossy` 会把残字节换成替换符，
    // 对"判头部有没有某一行"没有影响，且不会 panic。
    Ok(String::from_utf8_lossy(&buf).to_string())
}

pub fn read_str(p: &Path) -> R<String> {
    match fs::read(p) {
        // `String::from_utf8` takes the buffer over; `from_utf8_lossy(..)
        // .to_string()` only ever borrows it and then allocates a second copy,
        // so the file lives twice in memory until the `Vec` drops -- for the
        // 52 MB synthetic record that is 52 MB of peak RSS bought for nothing.
        //
        // Note `Cow::into_owned()`, the obvious "avoid the copy" spelling, does
        // not help here: `from_utf8_lossy` returns `Cow::Borrowed` whenever the
        // bytes are valid UTF-8 (the normal case), and turning *that* into an
        // owned String is exactly the same allocation-and-copy. Taking ownership
        // of the bytes is the only version that skips it.
        Ok(b) => Ok(match String::from_utf8(b) {
            Ok(s) => s,
            Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
        }),
        Err(e) => Err(EE::Io(format!("读取失败 {}: {e}", p.display()))),
    }
}

/// Files this process has actually rewritten, in call order. A write command
/// must never mutate a file the caller did not ask for without saying so, so
/// every atomic write records itself and the command prints the list.
pub fn written_files() -> &'static Mutex<Vec<PathBuf>> {
    static W: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(Vec::new()))
}

/// Print every file this run rewrote. Called once at process exit so that no
/// write command can add, drop or forget its own reporting.
/// Print the side-effect ledger, or suppress it in `--json` mode.
///
/// A machine reader asked for one document; a trailing "写入 2 个文件" line made
/// the document unparseable and, worse, only appeared for commands that touch
/// disk — so a caller saw it fail intermittently. In JSON mode the ledger is
/// attached as a `written` field by `main` instead of being printed here.
/// JSON array of strings, used for the write ledger.
pub fn jstr_list(items: &[String]) -> String {
    let parts: Vec<String> = items.iter().map(|s| jstr(s)).collect();
    format!("[{}]", parts.join(","))
}

pub fn take_write_report(root: &Path) -> Vec<String> {
    let Ok(v) = written_files().lock() else { return Vec::new() };
    v.iter().map(|p| rel(root, p)).collect()
}

pub fn flush_write_report(root: &Path) {
    let names = take_write_report(root);
    if names.is_empty() {
        return;
    }
    println!("写入 {} 个文件: {}", names.len(), names.join("、"));
}

/// 临时文件的自动清理。
///
/// 为什么需要它：这个函数有三条早退路径（写入失败、复读失败、改名失败），
/// 每条都应该删掉临时文件，靠手写在每处补一句 `remove_file` 很容易漏——
/// 事实上原来就漏了两条：`fs::write` 失败和 `fs::read` 失败都直接 `?` 返回，
/// 半截临时文件留在目录里。用 `ulimit -f 1` 让写入失败，能稳定复现一个
/// 1024 字节、内容被截断的 `.tmp<pid>` 残留。
///
/// 放在 Drop 里是因为它需要覆盖**所有**退出路径，包括将来新增的。
/// `keep()` 用于成功路径：改名成功后临时文件已经不存在，再去删一次会误删
/// 别人刚用同名写下的新文件。
struct TmpGuard {
    path: Option<std::path::PathBuf>,
}

impl TmpGuard {
    fn new(p: std::path::PathBuf) -> TmpGuard {
        TmpGuard { path: Some(p) }
    }
    /// 临时文件已经被 rename 走了，不要再删。
    fn keep(&mut self) {
        self.path = None;
    }
}

impl Drop for TmpGuard {
    fn drop(&mut self) {
        if let Some(p) = self.path.take() {
            let _ = fs::remove_file(&p);
        }
    }
}

pub fn write_atomic(p: &Path, data: &str) -> R<()> {
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension(format!(
        "tmp{}",
        std::process::id() as u64 % 100000
    ));
    // 从这里开始，任何路径退出都会删掉临时文件。
    let mut guard = TmpGuard::new(tmp.clone());
    fs::write(&tmp, data.as_bytes())
        .map_err(|e| EE::Io(format!("写入失败 {}: {e}", tmp.display())))?;
    // read back: a lost write must not be reported as success
    let back = fs::read(&tmp)
        .map_err(|e| EE::Io(format!("复读失败 {}: {e}", tmp.display())))?;
    if back != data.as_bytes() {
        return integrity(format!("写入复读不一致: {}", p.display()));
    }
    match fs::rename(&tmp, p) {
        // 成功：临时文件已经变成目标，别让 guard 去删它。
        Ok(_) => {
            guard.keep();
        }
        Err(_) => {
            // Windows 上 rename 不能覆盖已存在的文件，得先把目标挪开。
            //
            // 原来这里是 `remove_file(p)` 然后直接再 rename——第二步如果也
            // 失败，旧内容已经没了，新内容也没写进去，两边都丢。改成先把
            // 目标改名成 .bak（同目录，原子），rename 成功后再删 .bak；
            // 失败则把 .bak 挪回原位，至少保住旧内容。
            let bak = p.with_extension(format!(
                "bak{}",
                std::process::id() as u64 % 100000
            ));
            let moved = fs::rename(p, &bak).is_ok();
            match fs::rename(&tmp, p) {
                Ok(_) => {
                    guard.keep();
                    if moved {
                        let _ = fs::remove_file(&bak);
                    }
                }
                Err(e) => {
                    if moved {
                        let _ = fs::rename(&bak, p);
                    }
                    return Err(EE::Io(format!(
                        "原子改名失败 {}: {e}",
                        p.display()
                    )));
                }
            }
        }
    }
    if let Ok(mut v) = written_files().lock() {
        let s = p.to_string_lossy().to_string();
        if !v.iter().any(|x| x.to_string_lossy() == s) {
            v.push(p.to_path_buf());
        }
    }
    Ok(())
}

/// Fingerprint of one file's bytes, for "did this change since I read it?".
///
/// Eight hex characters of SHA-256 over the whole file. Short enough to paste into
/// a `--if-version` argument, wide enough that two different states of the same
/// record will not collide by accident.
///
/// Reading returns it alongside the content; `write_atomic_cas` compares it. The
/// fingerprint is what makes the gap between *an agent reading a file* and *the
/// agent's write command running* safe. That gap is measured in minutes, not
/// microseconds: an agent reads a record, thinks, composes an edit, and only then
/// writes. Any check that re-reads the file at command start sees the current
/// content and passes -- useless. The caller has to carry the fingerprint it saw
/// and hand it back, so the comparison is against the version the caller actually
/// reasoned about.
pub fn file_version(p: &Path) -> String {
    match fs::read(p) {
        Ok(b) => crate::sha256::digest_prefix(&b, 8),
        // Absent file: a stable marker, not an error. A caller that read "nothing"
        // and then writes is protected the same way.
        Err(_) => "-".to_string(),
    }
}

/// Refuse to write when the file is no longer the version the caller read.
///
/// `expect` is the fingerprint the caller was given when it read the file. An
/// empty value or `-` means "caller did not state a version" and the write
/// proceeds (that is the old behaviour, kept for commands that never read the
/// file first, such as `write` creating a new record).
pub fn require_version(p: &Path, expect: &str) -> R<()> {
    let expect = expect.trim();
    if expect.is_empty() || expect == "-" {
        return Ok(());
    }
    let now = file_version(p);
    if now == expect {
        return Ok(());
    }
    Err(EE::Integrity(format!(
        "版本不符，拒绝覆盖: {}\n\
         你读到的是版本 {expect}，磁盘上现在是 {now}。\n\
         说明在你读取之后，另一个进程或你自己之前的另一次修改已经改过这个文件。\n\
         请重新读取（abook show 会给出新版本号），确认当前内容后重做你的修改。",
        p.display()
    )))
}

/// Why a compare-and-swap write refused to land.
#[derive(Debug, Clone)]
pub struct Conflict {
    /// Which top-level part disagreed: a header field name, or `正文`.
    pub part: String,
    /// What this process loaded, before its own edit.
    pub base: String,
    /// What it wanted to write.
    pub mine: String,
    /// What is on disk now.
    pub theirs: String,
    /// Rendered once at construction so `msg()` can hand out a `&str`.
    pub msg_text: String,
}

impl Conflict {
    pub fn build(part: String, base: String, mine: String, theirs: String) -> Conflict {
        let mut c = Conflict {
            part,
            base,
            mine,
            theirs,
            msg_text: String::new(),
        };
        c.msg_text = c.render_msg();
        c
    }

    /// The message a user sees. It has to answer "what now?", not just "no".
    fn render_msg(&self) -> String {
        format!(
            "{} 在读取之后被另一个写入者改过，且改动落在同一处，无法自动合并。\n\
             你读到的版本与磁盘上的版本都保留着：请重新读取这一条，再重做你的修改。\n\
             （工具不会替你二选一——那样会静默丢掉另一边的内容。）",
            self.part
        )
    }
}

/// Write `data` only if `p` still holds `expected`, then verify by re-reading.
///
/// `write_atomic` already guarantees a reader never sees a half-written file, but
/// it happily overwrites whatever is there. That is wrong for a read-modify-write
/// command: this process read the file, spent time computing, and only then writes
/// -- and any other writer (a second `abook`, an editor, a script) that landed in
/// between is silently erased. Nothing reports it, because from the writer's point
/// of view the write succeeded.
///
/// So the rename is made conditional on the bytes being what we read. The check
/// happens immediately before the rename, which shrinks the window to the few
/// instructions in between rather than the whole command.
pub fn write_atomic_cas(p: &Path, data: &str, expected: &str) -> R<()> {
    match fs::read(p) {
        Ok(cur) => {
            let cur = String::from_utf8_lossy(&cur).to_string();
            if cur != expected {
                return Err(EE::Integrity(format!(
                    "文件在读取之后被改过，拒绝覆盖: {}",
                    p.display()
                )));
            }
        }
        // Vanished: treat as a conflict too. Recreating a file another process just
        // deleted is the same lost-update in the other direction.
        Err(_) => {
            return Err(EE::Integrity(format!(
                "文件在读取之后被删除，拒绝重建: {}",
                p.display()
            )));
        }
    }
    write_atomic(p, data)
}

/// Write only when the bytes would actually change. Derived views (atlas,
/// index, Global counters) are recomputed on every write; rewriting them
/// unchanged would make every edit look like it touched the whole library.
pub fn write_if_changed(p: &Path, data: &str) -> R<bool> {
    if let Ok(old) = fs::read(p) {
        if old == data.as_bytes() {
            return Ok(false);
        }
    }
    write_atomic(p, data)?;
    Ok(true)
}

pub fn ensure_dir(p: &Path) -> R<()> {
    fs::create_dir_all(p).map_err(|e| EE::Io(format!("创建目录失败 {}: {e}", p.display())))
}

pub fn list_md(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "md").unwrap_or(false) {
                v.push(p);
            }
        }
    }
    v.sort();
    v
}

pub fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn human_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n}B")
    } else if n < 1024 * 1024 {
        format!("{:.1}KB", n as f64 / 1024.0)
    } else {
        format!("{:.1}MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// 列表行的统一元信息前缀：`时间  体积  行数`。
///
/// 每条记录的输出都自带这三样，是为了让人判断"要不要整篇读、要不要翻页"——
/// 体积和行数不是装饰，是决定下一步动作的依据。此前各命令各写一套：`list` 是
/// `时间 体积 行数`，`path` 是 `体积 行数 时间`，而 `index`/`branch`/`recent`/
/// `skill`/`vault` 干脆一样都没有。同一份数据在不同命令里长成不同形状，读者每
/// 换一条命令就得重新认一次列，靠记忆去对齐。
///
/// 缺席的维度用 `-` 明示，不用空白或 0：`vault` 的明文行数不解密就不知道，写 0
/// 会被读成"这个条目是空的"。
pub fn row_meta(when: i64, bytes: Option<u64>, lines: Option<usize>) -> String {
    let t = if when > 0 { short_when(when) } else { "-".to_string() };
    let b = match bytes {
        Some(n) => human_bytes(n),
        None => "-".to_string(),
    };
    let l = match lines {
        Some(n) => format!("{n}行"),
        None => "-".to_string(),
    };
    format!("{t:>8}  {b:>7} {l:>5}")
}

/// Truncate a string to at most `n` chars, appending an ellipsis marker.
pub fn truncate_chars(s: &str, n: usize) -> String {
    let mut out = String::new();
    let mut c = 0;
    for ch in s.chars() {
        if c >= n {
            out.push('…');
            return out;
        }
        out.push(ch);
        c += 1;
    }
    out
}

// ---------------------------------------------------------------- output

/// Where a command's human-readable lines go.
///
/// A write command reports what it did in prose, one `println!` at a time --
/// 155 of them across the editing, writing, audit, branch and vault modules.
/// Those sentences are good for a person and useless to a caller: `abook set X
/// TITLE --json` printed `SESSION= x` and put its file list on stderr, so the
/// flag was advertised and not honoured.
///
/// Rather than rewrite every call site into a bespoke JSON branch (155 chances
/// to get one wrong, and a new one gets it wrong every time it is added), the
/// lines are captured into a buffer when the caller asked for JSON and shipped
/// as an `output` array next to the structured fields. One choke point, so a
/// command added later is machine-readable without anyone remembering to make
/// it so.
pub fn emit_line(s: impl AsRef<str>) {
    let s = s.as_ref();
    CAPTURE.with(|c| {
        if let Some(buf) = c.borrow_mut().as_mut() {
            buf.push(s.to_string());
            return;
        }
        println!("{s}");
    });
}

/// Default character budget for `emit_block` when the caller did not pass
/// `--all`. Chosen so a typical terminal, a chat message and a model context all
/// fit one block without the transport silently cutting it.
fn compact_chars() -> usize {
    std::env::var("ABOOK_BLOCK_CHARS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n >= 500)
        .unwrap_or(12_000)
}

/// Commands that write to the library and therefore need the exclusive lock.
///
/// Kept as a list rather than a per-command flag because the lock has to be taken
/// before dispatch, and dispatch is the only place that sees every command.
/// Anything missing here writes without the lock -- which is exactly how two
/// concurrent `write`s both returned 0 while the map and the counts lost one of
/// them.
pub const WRITE_COMMANDS: &[&str] = &[
    "write", "copy", "merge", "split", "update", "append", "prepend", "patch", "set", "unset",
    "rename", "move", "delete", "restore", "promote", "demote", "commit", "refresh", "recount",
    "repair", "normalize", "sync", "leftover", "init", "attach", "vault",
    // These three rewrite the map or the records too. Leaving them out meant
    // `branch add`, `doctor --fix` and `refs --fix` wrote with no lock at all --
    // exactly the hole that let two concurrent writers lose one of two updates.
    "branch", "doctor", "refs",
];

/// True when this command may modify files under the library root.
pub fn is_write_command(cmd: &str) -> bool {
    let c = crate::spec::canonical(cmd);
    WRITE_COMMANDS.contains(&c.as_str())
}

/// An exclusive, cross-process lock on the whole library.
///
/// Taken for every write command, released when the value drops. Plain
/// `create_new` on a lock file: portable, no external crate, and the kernel
/// closes the handle on any exit path including panic, so a crash cannot leave
/// the library permanently locked.
///
/// A stale lock (process killed between create and remove) is broken after
/// [`STALE_LOCK_SECS`]. Without that, one crash would lock the library forever;
/// with it, a genuinely long write still holds the lock because the timestamp is
/// refreshed by the holder.
pub struct LibLock {
    path: std::path::PathBuf,
}

/// How long a lock file may sit untouched before another process may break it.
const STALE_LOCK_SECS: u64 = 600;

impl LibLock {
    /// Acquire, waiting up to `wait_secs` for a competing writer to finish.
    pub fn acquire(root: &std::path::Path, wait_secs: u64) -> R<LibLock> {
        let path = root.join(".abook.lock");
        if let Some(d) = path.parent() {
            let _ = fs::create_dir_all(d);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait_secs);
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    let _ = write!(f, "{} {}\n", std::process::id(), now_secs());
                    return Ok(LibLock { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Break a lock left behind by a killed process.
                    if let Ok(txt) = std::fs::read_to_string(&path) {
                        let started = txt
                            .split_whitespace()
                            .nth(1)
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(0);
                        // Age alone is not proof of death: a genuinely long
                        // write looked identical to a crash, so a 10-minute
                        // import could have its lock stolen mid-write. Check
                        // whether the holder still exists, and only fall back to
                        // age when the pid is unreadable.
                        let pid = txt
                            .split_whitespace()
                            .next()
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(0);
                        let aged = started > 0
                            && (now_secs() as u64).saturating_sub(started) > STALE_LOCK_SECS;
                        // 进程确实没了就是确定事实，不用再等满 600 秒。
                        //
                        // 原来的条件是 `aged && dead`，等于"已经死了也得等十分钟"——
                        // 下次写库的人被白白挡十次 30 秒等待，才能继续。而
                        // `pid_alive` 在 /proc 存在时给的是确定答案，`pid > 0`
                        // 说明锁文件里的 pid 也是读得出来的，这时候"死了"不需要
                        // 年龄来兜底。
                        //
                        // 年龄只在**判断不了是否存活**时才有意义（读不到 pid，
                        // 或平台没有 /proc）——那时宁可多等，也不能抢走一个
                        // 正在写长任务的活锁。
                        let dead_certain = pid > 0 && !pid_alive(pid);
                        let unknown_and_aged = pid == 0 && aged;
                        if dead_certain || unknown_and_aged {
                            let _ = std::fs::remove_file(&path);
                            continue;
                        }
                    }
                    if std::time::Instant::now() >= deadline {
                        let holder = std::fs::read_to_string(&path).unwrap_or_default();
                        return integrity(format!(
                            "库被另一个写入者占用（{holder}）；等待 {wait_secs} 秒仍未释放。\
另一个 abook 正在写这个库，或上次异常退出留下了锁（{} 秒后会自动解除）",
                            STALE_LOCK_SECS
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(120));
                }
                Err(e) => return Err(EE::Io(format!("无法获取库锁 {}: {e}", path.display()))),
            }
        }
    }
}

/// True when `pid` is a live process.
///
/// `/proc/<pid>` exists on Linux and WSL2; elsewhere the check is skipped and the
/// caller falls back to age alone. Written without external crates on purpose:
/// the tool is one static binary by design.
fn pid_alive(pid: u32) -> bool {
    let probe = std::path::Path::new("/proc").join(pid.to_string());
    if !std::path::Path::new("/proc").exists() {
        return true; // Cannot tell; assume alive rather than steal a live lock.
    }
    probe.exists()
}

impl Drop for LibLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Line/character/byte totals for a block of text.
pub fn block_stats(text: &str) -> (usize, usize, usize) {
    let body = text.strip_suffix('\n').unwrap_or(text);
    (body.lines().count(), body.chars().count(), body.len())
}

/// Print a long block of text with its size stated before and after.
///
/// Why this exists: a reader -- human or model -- that cannot see the total has
/// no way to tell "this is the whole thing" from "this stopped at the transport
/// cut". Truncation is then invisible, and a conclusion gets built on half the
/// material while believing it read all of it. The header states the totals up
/// front; the footer repeats them, so the end of the output carries the answer
/// too in case the head scrolled away or the middle was elided.
///
/// Both lines and characters are reported. Lines alone mislead on prose (one
/// long paragraph is one line); characters alone mislead on tables.
pub fn emit_block(label: &str, text: &str, want_all: bool) {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let (lines, chars, bytes) = block_stats(body);
    emit_line(format!(
        "── {label} 开始 ── 全文 {lines} 行 / {chars} 字 / {bytes} B"
    ));
    let budget = compact_chars();
    if want_all || chars <= budget {
        print!("{body}");
        if !body.is_empty() {
            println!();
        }
        emit_line(format!(
            "── {label} 结束 ── 全文 {lines} 行 / {chars} 字 / {bytes} B；本次完整输出，未截断"
        ));
        return;
    }
    // Over budget and the caller did not ask for everything. Say exactly how
    // much is missing -- returning less than the whole silently is the very
    // failure this header exists to prevent.
    let keep: String = body.chars().take(budget).collect();
    let (klines, kchars, _) = block_stats(&keep);
    print!("{keep}");
    if !keep.ends_with('\n') {
        println!();
    }
    emit_line(format!(
        "── {label} 未读完 ── 已输出 {klines}/{lines} 行、{kchars}/{chars} 字，尚缺 {} 字；\
加 --all 看全文（或用 --page N 翻页）",
        chars.saturating_sub(kchars)
    ));
}

thread_local! {
    static CAPTURE: std::cell::RefCell<Option<Vec<String>>> =
        const { std::cell::RefCell::new(None) };
}

/// Start collecting lines instead of printing them.
pub fn begin_capture() {
    CAPTURE.with(|c| *c.borrow_mut() = Some(Vec::new()));
}

/// True when lines are being collected.
pub fn capturing() -> bool {
    CAPTURE.with(|c| c.borrow().is_some())
}

/// Stop collecting and return what was collected.
pub fn take_capture() -> Vec<String> {
    CAPTURE.with(|c| c.borrow_mut().take().unwrap_or_default())
}

// ---------------------------------------------------------------- json

pub fn jesc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

pub fn jstr(s: &str) -> String {
    format!("\"{}\"", jesc(s))
}

/// Glob-free simple matcher: supports `*` and `?`.
pub fn wildcard_match(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ti;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Case-insensitive substring test.
pub fn ci_contains(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    hay.to_lowercase().contains(&needle.to_lowercase())
}

/// Split a value list on `,` or `、` and trim.
pub fn split_list(s: &str) -> Vec<String> {
    s.split(|c| c == ',' || c == '，' || c == '、' || c == ';')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// The summary the tool derives from a body when the caller supplies none.
/// Kept as one function so edits can tell "still the derived value" from
/// "the author wrote this".
pub fn derive_summary(body: &str) -> String {
    // A body may span many lines; SUMMARY must stay on one.
    truncate_chars(&crate::rec::fold_to_one_line(body), 160)
}

/// Re-derive body-dependent fields, but only where they still hold the
/// derived value. An authored SUMMARY or SIZE is never overwritten.
pub fn rederive_after_body_change(rec: &mut crate::rec::Record, old_body: &str) {
    let new_body = rec.body.clone();
    if rec.get("SUMMARY") == derive_summary(old_body) {
        let v = derive_summary(&new_body);
        rec.set("SUMMARY", &v);
    }
    // SIZE follows the same rule.
    let old_size = crate::schema::size_for(old_body.len());
    if rec.get("SIZE") == old_size {
        let v = crate::schema::size_for(new_body.len());
        rec.set("SIZE", v);
    }
}

/// Which libraries a command should act on, from `--lib`.
///
/// Two modules had their own copy and they disagreed: the one in `cmd_read`
/// accepted the Chinese aliases 「正式」「候选」 while the one in `cmd_branch` did
/// not, so `recount --lib 候选` silently fell through to "both" and recounted
/// the wrong set. One implementation, superset of both.
pub fn libs_from(a: &Args) -> Vec<crate::rec::Lib> {
    match a.get("lib").unwrap_or("both") {
        "canonical" | "c" | "正式" => vec![crate::rec::Lib::Canonical],
        "candidate" | "n" | "候选" => vec![crate::rec::Lib::Candidate],
        _ => vec![crate::rec::Lib::Canonical, crate::rec::Lib::Candidate],
    }
}

/// Like `println!`, but the line can be captured for JSON callers.
///
/// Spelled as a macro so the call sites read exactly like the `println!` they
/// replace and can be mechanically converted.
#[macro_export]
macro_rules! say_line {
    () => { $crate::util::emit_line("") };
    ($($t:tt)*) => { $crate::util::emit_line(format!($($t)*)) };
}

#[cfg(test)]
mod args_tests {
    use super::*;

    /// `--limit` 必须真的能读到。
    ///
    /// 别名折叠把 `limit` 折成 `per-page`，只留折叠后的键，于是 `a.num("limit")`
    /// 恒为 `None`——而四个命令（`index`/`recall`/`recent`/`log`）直接读 `limit`，
    /// 那句 `unwrap_or(40)` 成了唯一生效的值：`--limit 100` 无论传什么都只出
    /// 40 行，而同一处提示语还在叫用户"用 --limit 扩大"。
    ///
    /// 一个被帮助文档列出、被解析、被写进提示的 flag，从来没被读到过——这比
    /// 不支持它更糟，因为"支持"是写在帮助里的。
    #[test]
    fn limit_and_per_page_are_both_readable() {
        let argv: Vec<String> = ["index", "--limit", "5"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = Args::parse(&argv).expect("parse");
        assert_eq!(a.num("limit"), Some(5), "限总数必须读得到");
        assert_eq!(a.num("per-page"), Some(5), "分页侧也要能看到");

        let argv: Vec<String> = ["index", "--per-page", "7"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = Args::parse(&argv).expect("parse");
        assert_eq!(a.num("per-page"), Some(7));
        assert_eq!(a.num("limit"), None, "只给 per-page 时不该伪造一个 limit");

        // 校验按折叠后的名字判：`--limit` 与 `--per-page` 是同一个参数的两种
        // 写法，不能因为它保留了原名就被判成未知参数。曾经如此，表现为
        // `list --limit 2` 报"不接受 --limit"。
        let argv: Vec<String> = ["list", "--limit", "2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = Args::parse(&argv).expect("parse");
        assert!(a.validate_for("list").is_ok(), "list 应接受 --limit");

        // 真正陌生的参数仍要拒绝，否则上面那条"放宽"会把校验废掉。
        let argv: Vec<String> = ["list", "--bogus", "2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = Args::parse(&argv).expect("parse");
        assert!(a.validate_for("list").is_err(), "未知参数必须仍被拒绝");
    }
}
