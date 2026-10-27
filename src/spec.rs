//! The per-command parameter table, in one place.
//!
//! Before this module the only description of "which flags does command X
//! accept" lived inside the help prose. That is fine for a human and hopeless
//! for a caller: `list --json` was accepted, silently ignored, and returned
//! human text, so a machine reader parsed nothing and believed it had data.
//! The help screen, the parameter check and `describe` now all read this table,
//! so a flag cannot be advertised in one place and missing in another.

/// (command, value parameters, bool flags, summary)
///
/// `params` are the ones that consume the next token; `flags` are the bare
/// switches. Both are validated by the parser before the command runs.
pub const SPEC: &[(&str, &str, &str, &str, &str)] = &[
    ("write", "title,read-when,not-for,body-file,lib,approve,dry-run,force,body", "approve,dry-run,force,json", "新建记录，带格式校验", "--title 示例 --body-file @file --lib candidate"),
    ("update", "if-version,title,body,body-file,approve,dry-run", "approve,dry-run,json", "改已有记录的头部或正文", "@any --title 示例"),
    ("set", "if-version,value,dry-run,approve", "dry-run,json", "改单个字段", "@any TITLE \"新标题\" --approve"),
    ("unset", "if-version,text,file,body,approve,dry-run", "dry-run,json", "删正文里的一段", "@any --text x"),
    ("append", "if-version,text,file,dry-run,approve,body", "dry-run,json", "正文末尾追加", "@any --text x"),
    ("prepend", "if-version,text,dry-run,approve,body,file", "dry-run,json", "正文开头插入", "@any --text x"),
    ("patch", "if-version,old,new,field,in,all,dry-run,approve", "all,dry-run,json", "按文本替换，可限定字段", "@any --old x --new y"),
    ("rename", "if-version,title,dry-run,approve", "dry-run,json", "改标题", "@any --title 新标题"),
    ("move", "if-version,dry-run,approve", "dry-run,json", "换分支", "@any"),
    ("copy", "to,approve,dry-run", "approve,dry-run,json", "复制成新记录", "@any --to candidate"),
    ("merge", "title,approve,dry-run,remove-sources", "remove-sources,approve,dry-run,json", "多条合一", "@any @any --title 合并"),
    ("split", "at,dry-run,approve", "dry-run,json", "一条拆成多条", "@any --at 1"),
    ("delete", "if-version,approve,dry-run,hard", "hard,approve,dry-run,json", "标 DELETED，--hard 才物理删", "@any"),
    ("restore", "if-version,approve,dry-run", "approve,dry-run,json", "恢复被删记录", "@any"),
    ("promote", "branch,approve,dry-run,force,all,kind,maturity,q", "approve,force,dry-run,json,all", "候选升正式（--all 批量）", "@any"),
    ("demote", "approve,dry-run", "approve,dry-run,json", "正式降候选", "@any"),
    ("show", "field,fields,page,per-page,deviations,kb,size", "deviations,json,all,full,whole", "全字段 + 正文（默认按 30KB 预算分页）", "@any"),
    ("cat", "page,per-page,kb,size", "json,all,full,whole", "只打印正文（默认按 30KB 预算分页）", "@any"),
    ("list", "lib,kind,status,maturity,tag,q,page,per-page,include-promoted,project,title", "json", "列表，分页", "--limit 2"),
    ("get", "field,page,per-page", "json", "取单字段", "@any --field TITLE"),
    ("diff", "page,per-page", "json", "两条记录差异", "@any @any"),
    ("recall", "q,limit,page,per-page,all,body,include-promoted,raw,lib", "all,body,raw,include-promoted,json", "召回", "--limit 2"),
    ("export", "lib,format,out,include-deleted,page,per-page", "include-deleted,json", "导出", "--format json"),
    ("index", "limit,page,per-page", "json", "Index.md 路由", "--limit 2"),
    ("atlas", "card,page,per-page", "json", "地图分支", ""),
    ("recent", "limit,page,per-page", "json", "最近变更", "--limit 2"),
    ("log", "limit,page,per-page", "json", "变更记录", "--limit 2"),
    ("history", "limit,page,per-page", "json", "某条记录的变更", "@any --limit 2"),
    ("staging", "page,per-page,show,decision,ref,dry-run", "json,dry-run", "待决暂存件；resolve <名> --decision 结论 结清一件", "resolve <名> --decision 保持现状 --ref N-2026..."),
    ("review", "page,per-page,show", "json", "晋升记录", "--limit 2"),
    ("tree", "lib,page,per-page,depth", "json,flat,no-tree", "分支树（有 PARENT 时按层级缩进）", "--flat"),
    ("path", "", "json", "反查记录所属分支链；给分支名则向下展开子树", "@任一条记录ID 或 分支名"),
    ("attach", "VAULT_SUB", "json,force,dry-run,move,all", "附件（文件域）：list/add/get/rm/path/check", "list <记录>"),
    ("stats", "lib", "json", "统计", ""),
    ("skill", "root,title,body,body-file,reason", "approve,force,dry-run,json", "Skill 约定与候选管理；子命令见 skill guide", "guide"),
    ("spec", "root", "json", "取回被抽离的规范全文；不带名字列目录", "plan"),
    ("state", "", "json", "库状态", ""),
    ("paths", "", "json", "各目录绝对路径", ""),
    ("budget", "", "json", "读取量估算", ""),
    // `search` had no row at all, so the per-command check never ran for it:
    // every flag in the global set was accepted, including flags belonging to
    // other commands. `describe search` answered "没有命令 search" for a command
    // that `commands` listed and that works fine.
    (
        "search",
        "and,field,lib,context,distance,level,max,min-score,page,per-page",
        "regex,re,fuzzy,approx,literal,exact,case,raw-lines,lines,whole-line,semantic,sem,ask,deep,no-expand,include-deleted,hide-promoted,json",
        "全文检索：字面/正则/模糊/语义",
        "标题 --limit 5 --json",
    ),
    ("grep", "context,page,per-page", "json", "正则直搜", "GATE"),
    ("identity", "page,per-page", "json", "凭据身份索引（不含值）", "GitHub"),
    ("vault", "approve,body,body-file,dry-run,from,id,out,purpose,purpose-only,quiet,rebuild,stdin", "json", "凭据保险箱：ls get put rm ledger export import verify rekey lint repair names", "ls"),
    ("recount", "lib,apply,dry-run,page,per-page", "apply,dry-run,json", "按磁盘修正 COUNT/MAXSIZE", "--apply"),
    ("refresh", "lib,apply,dry-run,page,per-page", "apply,dry-run,json", "按磁盘重建地图路径", "--apply"),
    ("normalize", "lib,apply,dry-run,page,per-page", "apply,dry-run,json", "旧命名升级为新命名", "--apply"),
    ("init", "lib", "json", "建库骨架", "canonical"),
    ("commit", "text,file,slug,body,body-file", "json,dry-run", "写变更记录", "--slug t --body x"),
    ("audit", "lib,all", "all,json", "全面体检", ""),
    ("check", "lib", "json", "格式校验，不改文件", ""),
    (
        "refs",
        "lib",
        "fix,approve,dry-run,json",
        "检查引用是否指得着；--fix 只修能唯一推导的",
        "--fix --dry-run",
    ),
    ("lint", "lib", "json", "风格检查", ""),
    ("doctor", "lib,fix,dry-run", "fix,apply,dry-run,json", "诊断，--fix 就地修（--dry-run 只看不改）", "--fix --dry-run"),
    ("reconcile", "lib,dry-run", "fix,apply,dry-run,json", "地图与磁盘对账（--dry-run 只看不改）", "reconcile --dry-run"),
    ("repair", "lib,apply,dry-run,page,per-page,only", "apply,dry-run,json", "按校验结果修，--only 限定类别", "--apply --only refs"),
    ("leftover", "apply,lib,json", "json", "残留文件", "abook leftover --apply"),
    ("branch", "branch,keys,read-when,page,per-page", "json,dry-run,force", "分支卡片增删改", "abook branch list"),
    ("describe", "page,per-page", "json", "命令契约（本表）", "list"),
    ("commands", "page,per-page", "json", "命令名列表", "--per-page 5"),
    ("help", "", "json", "帮助", ""),
    ("version", "", "json", "版本", ""),
    ("detect", "", "json", "安装位置探测", ""),
    ("install", "url,dir,dry-run,token", "dry-run,json", "自安装到 PATH", "--dry-run"),
    ("upgrade", "url,dir,check", "check,force,dry-run,json", "自更新到最新版", "--check"),
    ("selftest", "only", "all,json,verbose", "内置用例；--only N 只跑第 N 条", "selftest --only 1"),

];

/// (command, positional form)
///
/// The row above answers "which flags does this take". It does not answer
/// "what goes after the command name", and for a write command that is the one
/// thing a caller gets wrong. `describe set` used to print `--value` only, so
/// the obvious reading -- `set --id X --field TITLE --value Y` -- failed with
/// `缺少参数：记录`, and nothing told the caller that `set` wants three
/// positionals. The help prose had it right all along; `describe` is what an
/// agent actually reads.
///
/// The forms below are copied from that prose, not invented: `rename` really
/// does take a new name as a second positional, `update` and `patch` really do
/// take the record first.
pub const POSITIONALS: &[(&str, &str)] = &[
    ("describe", "<命令>"),
    ("cat", "<记录>"),
    ("search", "<词> [更多词]"),
    ("edit", "<记录>"),
    ("set", "<记录> <字段> <值>"),
    ("unset", "<记录> <字段>"),
    ("append", "<记录>"),
    ("prepend", "<记录>"),
    ("patch", "<记录>"),
    ("update", "<记录>"),
    ("rename", "<记录> <新名>"),
    ("move", "<记录> <分支>"),
    ("copy", "<记录>"),
    ("merge", "<记录A> <记录B> [更多]"),
    ("split", "<记录>"),
    ("delete", "<记录>"),
    ("restore", "<记录>"),
    // 位置参数是可选的：`promote --all` 不需要指定单条候选。写成 `<候选>`
    // 会让 `describe promote` 报一个必填参数，而 `--all` 恰恰不该要它。
    ("promote", "[候选]"),
    ("demote", "<正式>"),
    ("show", "<记录>"),
    ("get", "<记录>"),
    ("diff", "<记录> <记录>"),
    ("history", "<记录>"),
    ("budget", "[记录]"),
];

/// Positional form for one command, empty when it takes none.
pub fn positionals(cmd: &str) -> &'static str {
    POSITIONALS
        .iter()
        .find(|(c, _)| *c == cmd)
        .map(|(_, f)| *f)
        .unwrap_or("")
}

/// Look up one command. Returns (canonical name, comma-joined params, summary).
pub fn spec_for(cmd: &str) -> Option<(String, String, String)> {
    let c = canonical(cmd);
    SPEC.iter().find(|(n, _, _, _, _)| *n == c).map(|(n, p, f, s, _)| {
        let mut all: Vec<&str> = Vec::new();
        if !p.is_empty() {
            all.extend(p.split(','));
        }
        if !f.is_empty() {
            all.extend(f.split(','));
        }
        // Paging is NOT appended globally any more.
        //
        // It used to be, on the theory that every list command pages. Only 15 of
        // 70 actually read the flags; the other 55 accepted them in silence and
        // ignored them, and `describe set` cheerfully advertised "--page /
        // --per-page（默认 200）" for a command that writes one field and has
        // nothing to page. A caller who trusts that line adds `--per-page 5` to
        // a `set` and cannot tell from the exit code that it did nothing.
        // Rows that page list `page,per-page` themselves; the rest now reject
        // the flags outright, which is the honest answer.
        // A name may appear both as a value-taking param and as a boolean flag
        // (`approve`, `dry-run`, `force` do), and printing it twice made
        // `describe` look broken rather than informative.
        let mut deduped: Vec<&str> = Vec::new();
        for x in all {
            if !deduped.contains(&x) {
                deduped.push(x);
            }
        }
        let all = deduped;
        (n.to_string(), all.join(" --"), s.to_string())
    })
}

/// Aliases are resolved before the spec lookup, so `ls` answers with `list`.
pub fn canonical(cmd: &str) -> String {
    match cmd {
        "ls" => "list",
        "get" => "get",
        "sync" => "refresh",
        "fix" => "repair",
        "where" => "detect",
        "self-install" => "install",
        "self-update" => "upgrade",
        "mv" => "move",
        "rm" => "delete",
        // Aliases that were dispatched but not mapped here: parameter
        // validation looked them up by their alias name, found no spec row and
        // waved every flag through, so `abook add --typo x` was accepted.
        "add" => "write",
        "edit" => "update",
        "find" => "search",
        "cp" => "copy",
        "map" => "atlas",
        "index-search" => "search",
        other => other,
    }
    .to_string()
}

pub fn takes_json(cmd: &str) -> bool {
    let c = canonical(cmd);
    SPEC
        .iter()
        .find(|(n, _, _, _, _)| *n == c)
        .map(|(_, _, f, _, _)| f.split(',').any(|x| x == "json"))
        .unwrap_or(false)
}

/// Does this command accept `flag` as a value parameter or a bare switch?
///
/// `describe` used to answer "does this command paginate" by re-splitting the
/// display string it had just built, so the answer lived in two places at once.
/// One lookup here keeps the table the only source.
pub fn takes_flag(cmd: &str, flag: &str) -> bool {
    let c = canonical(cmd);
    SPEC.iter()
        .find(|(n, _, _, _, _)| *n == c)
        .map(|(_, p, f, _, _)| {
            p.split(',').any(|x| x == flag) || f.split(',').any(|x| x == flag)
        })
        .unwrap_or(false)
}

/// Every command name this table knows, sorted.
pub fn names() -> Vec<String> {
    let mut v: Vec<String> = SPEC.iter().map(|(n, _, _, _, _)| n.to_string()).collect();
    v.sort();
    v
}

/// The parameters a command accepts, for the parser's pre-run check.
pub fn params_of(cmd: &str) -> Option<Vec<String>> {
    if let Some((_, p, _, _, _)) = SPEC.iter().find(|(n, _, _, _, _)| {
        *n == canonical(cmd)
    }) {
        // A command whose parameters belong to its subcommands cannot be
        // checked at this level without rejecting every legitimate flag.
        if *p == "VAULT_SUB" {
            return None;
        }
    }
    spec_for(cmd).map(|(_, p, _)| {
        p.split(" --")
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    })
}
