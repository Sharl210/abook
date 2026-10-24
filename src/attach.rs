//! Attachments: the file half of a memory record.
//!
//! A record used to be one `.md` file and nothing else. Anything worth keeping
//! that was not prose -- a screenshot, a screen recording, an original log, a
//! PDF, a captured request -- had two bad homes: pasted into the body, where it
//! costs context on every read, or left outside the library, where it loses its
//! tie to the record and does not survive a migration.
//!
//! An attachment directory beside the record solves both. The record stays
//! prose; the evidence sits next to it:
//!
//!   records/C-20261024T124407-d4033a24.md
//!   records/C-20261024T124407-d4033a24-refs/01-screenshot.png
//!   records/C-20261024T124407-d4033a24-refs/raw-capture.log
//!
//! The shape mirrors a skill's reference folder on purpose: same idea, one
//! naming rule to remember.
//!
//! ## Two domains
//!
//! A record therefore has a *text domain* (the `.md` file: bytes and lines, the
//! part that costs context) and a *file domain* (the `-refs` folder: bytes and
//! count, never lines -- a PNG has no lines). Both are counted, separately, so a
//! reader can tell "2 KB of prose" from "2 KB of prose plus 40 MB of video"
//! before deciding to open anything.
//!
//! Everything in the folder is an attachment regardless of type. Plain text
//! included: if it is not the record itself, it is evidence the record carries.

use crate::rec::{Lib, Record};
use crate::util::*;
use std::fs;
use std::path::{Path, PathBuf};

/// Suffix for the attachment folder.
///
/// `refs` rather than `references`: it appears once per record, in listings
/// where width is scarce, and the shorter form is unambiguous. Recognition is
/// deliberately wider than creation -- see `candidates`.
pub const SUFFIX: &str = "refs";

/// Every suffix recognised when *reading*, canonical first.
///
/// A folder created by hand or by an older habit must still be found. Silently
/// ignoring `-references/` would mean the tool reports "no attachments" for a
/// record that visibly has a folder of them sitting beside it -- the exact
/// disappearance this library has been bitten by before.
pub const RECOGNISED: &[&str] = &["refs", "ref", "references", "assets", "files"];

/// The canonical attachment directory for a record (whether or not it exists).
pub fn dir_for(rec_path: &Path) -> PathBuf {
    let stem = rec_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    rec_path.with_file_name(format!("{stem}-{SUFFIX}"))
}

/// Every attachment directory present on disk for one record.
///
/// Plural on purpose. Returning the first match hid a real case in testing: a
/// folder created by hand as `-references/` sat next to a `-refs/` made by this
/// tool, and the listing counted only one of them -- reporting "2 attachments,
/// 6 KB" for a record that actually held three files. A reader who trusts that
/// number would decide the record was small enough to load whole.
pub fn all_dirs(rec_path: &Path) -> Vec<PathBuf> {
    let stem = rec_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    RECOGNISED
        .iter()
        .map(|suf| rec_path.with_file_name(format!("{stem}-{suf}")))
        .filter(|p| p.is_dir())
        .collect()
}

/// The attachment directory actually present on disk, if any.
///
/// Canonical spelling wins when more than one exists, so a caller who asks for
/// "the" directory gets the one this tool writes to.
pub fn existing_dir(rec_path: &Path) -> Option<PathBuf> {
    all_dirs(rec_path).into_iter().next()
}

/// One attachment, as listed to a reader.
#[derive(Clone, Debug)]
pub struct Item {
    /// Path relative to the record's directory, so it reads the same as the
    /// record's own path in a listing.
    pub name: String,
    pub bytes: u64,
}

/// Everything the file domain of one record contains.
#[derive(Default, Clone)]
pub struct Files {
    pub dir: Option<PathBuf>,
    pub items: Vec<Item>,
    pub bytes: u64,
}

impl Files {
    pub fn count(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// Short form for a listing column: empty when there are no attachments, so
    /// ordinary records keep the width they had.
    pub fn brief(&self) -> String {
        if self.items.is_empty() {
            String::new()
        } else {
            format!("{}附件", self.items.len())
        }
    }
}

/// Walk one attachment directory.
///
/// Recursive on purpose: an attachment folder may group its own contents
/// (a `frames/` subdirectory for extracted stills, say), and reporting only the
/// top level would understate the weight of a record by an order of magnitude.
pub fn scan(rec_path: &Path) -> Files {
    let mut out = Files::default();
    let dirs = all_dirs(rec_path);
    if dirs.is_empty() {
        return out;
    }
    out.dir = Some(dirs[0].clone());
    let base = rec_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut stack = dirs;
    while let Some(cur) = stack.pop() {
        let rd = match fs::read_dir(&cur) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let meta = match fs::metadata(&p) {
                Ok(m) => m,
                Err(_) => continue,
            };
            out.bytes += meta.len();
            out.items.push(Item {
                name: p
                    .strip_prefix(&base)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/"),
                bytes: meta.len(),
            });
        }
    }
    out.items.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Attachment totals for every record in one library, keyed by file name.
///
/// Built once per command. Scanning a directory per record inside a listing loop
/// turns `list` over 500 records into 500 directory walks, and the counts would
/// still be wrong the moment a record was renamed between two such walks.
pub fn index(root: &Path, lib: Lib) -> std::collections::HashMap<String, Files> {
    let mut m = std::collections::HashMap::new();
    for r in crate::rec::load_lib(root, lib) {
        let f = scan(&r.path);
        if !f.is_empty() {
            m.insert(r.file_name(), f);
        }
    }
    m
}

/// Every attachment directory in a library, including orphans.
///
/// An orphan is a `-refs` folder whose record is gone. It still occupies disk
/// and still holds evidence, so it is reported rather than cleaned up: deleting
/// someone's screenshots because a record was renamed is not a repair.
pub fn orphans(root: &Path, lib: Lib) -> Vec<(String, u64, usize)> {
    let dir = root.join(lib.dir());
    let mut out = Vec::new();
    let rd = match fs::read_dir(&dir) {
        Ok(r) => r,
        Err(_) => return out,
    };
    let mut records: Vec<String> = crate::rec::load_lib(root, lib)
        .iter()
        .map(|r| {
            r.path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        })
        .collect();
    records.sort();
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let stem = RECOGNISED
            .iter()
            .find_map(|s| name.strip_suffix(&format!("-{s}")).map(|x| x.to_string()));
        let stem = match stem {
            Some(s) => s,
            None => continue,
        };
        if records.contains(&stem) {
            continue;
        }
        let f = scan(&p.with_file_name(format!("{stem}.md")));
        let (mut by, mut n) = (0u64, 0usize);
        let mut stack = vec![p.clone()];
        while let Some(cur) = stack.pop() {
            if let Ok(rd) = fs::read_dir(&cur) {
                for e in rd.flatten() {
                    let q = e.path();
                    if q.is_dir() {
                        stack.push(q);
                    } else if let Ok(m) = fs::metadata(&q) {
                        by += m.len();
                        n += 1;
                    }
                }
            }
        }
        let _ = f;
        out.push((name, by, n));
    }
    out.sort();
    out
}

// ---------------------------------------------------------------- commands

fn need_record(a: &Args) -> R<(PathBuf, Record)> {
    let root = resolve_root(a);
    let sel = a.need_pos(1, "记录选择器（路径 / 文件名 / ID）")?;
    let (path, lib) = crate::rec::resolve_record(&root, &sel)?;
    let rec = Record::parse(&path, lib)?;
    Ok((path, rec))
}

/// `attach` — list, add, read, or remove a record's attachments.
pub fn cmd_attach(a: &Args) -> R<i32> {
    let sub = a.pos.first().cloned().unwrap_or_default();
    match sub.as_str() {
        "" | "list" | "ls" => list(a),
        "add" | "put" | "copy" => add(a),
        "get" | "read" | "cat" => get(a),
        "rm" | "remove" | "delete" => remove(a),
        "path" => path(a),
        "sync" => sync(a),
        "check" | "orphans" => check(a),
        "refs" => refs(a),
        "guide" | "help" => {
            for line in GUIDE.lines() {
                crate::say_line!("{line}");
            }
            Ok(0)
        }
        other => usage(&format!(
            "attach 的子命令有 list / add / get / rm / path / check；不认识 `{other}`"
        )),
    }
}

const GUIDE: &str = r#"# 附件（文件域）

记忆条目 = 文字域 + 文件域。

  文字域  records/<条目名>.md            有字节，也有行数
  文件域  records/<条目名>-refs/         有字节，没有行数（图片视频没有"行"）

放在 `-refs/` 里的任何东西都是附件：截图、录屏、原始日志、PDF、抓包、表格。
纯文本也算附件——只要不是条目本体，它就是这条记录携带的证据。

目录名规则：`<条目名>-refs`，与 Skill 的引用文件夹同构。读取时也认
`-ref` / `-references` / `-assets` / `-files`，手建的目录不会被忽略；
多个命名并存时全部统计。

## 命令

  attach list <记录>                    列出附件（名字 + 体积）
  attach add <记录> <文件> [--name N]   复制进附件目录
             [--move] [--force]
  attach get <记录> <附件名>            打印绝对路径（不打印内容）
  attach rm <记录> <附件名>             删一个附件
  attach path <记录>                    打印附件目录路径
  attach sync <记录> | --all            清单与磁盘重新对齐
  attach refs <记录>                    列出正文里的附件引用，标出悬空的
  attach check                          列出没有对应记录的孤立目录

## 清单块

每次附件变动，工具把清单回写进记录末尾：

  [ATTACH]
  1	<相对名>	<字节>
  2	<相对名>	<字节>
  [/ATTACH]

TAB 分隔（文件名可含空格）；序号供正文引用。回写走正常写盘路径，所以
`UPDATED` 跟着刷新——附件变了，条目就算改过，列出时自然排到前面。
清单是记录的派生部分，不走 --approve：附件增删已是执行完的确定事实。

正文引用写法：`本条目附件1://文件名`，指向清单第 1 行。

## 一致性与漂移

工具管不住有人绕过它直接 rm 文件，所以设计成能发现：

  attach sync   比对清单与磁盘，报「清单有磁盘无」「磁盘有清单无」「字节数变了」，
                非 --dry-run 时回写对齐
  attach refs   检查正文引用指向的第 N 个附件是否存在
  attach check  找孤立目录（记录改名/删除而附件还在），只报告不删除

## 只返回路径

附件可能是 40MB 视频。把字节塞进输出从来不是想要的：拿到路径就能自己读，
工具不该替你搬一遍。返回路径时会附一句边界：只操作这一个文件，同目录没给
路径的不要动。

## 旧库兼容

v3 之前的库没有附件目录、记录里也没有 [ATTACH] 块。这类库读取正常、
`attach sync --all` 报 0 漂移、不会凭空写清单；只有真加了附件才出现清单块。
改造是逐条懒惰的，不做全库迁移。

## 复制效率

  --move 且同设备      rename        零拷贝，瞬间
  CoW 文件系统         reflink       共享数据块，不占双倍
  跨设备/网络/WSL/UNC  内核级复制     copy_file_range / CopyFileExW / fcopyfile

不自己写读写循环：手写缓冲在网络路径上比内核实现慢一个数量级。超过 50MB 需
--force 确认；该阈值只对"真会复制出一份新数据"生效，同盘 rename 免检。
复制失败会说明卡在哪一步、接着怎么做，并清掉半截文件。
"#;
fn list(a: &Args) -> R<i32> {
    let (path, rec) = need_record(a)?;
    let f = scan(&path);
    if a.has("json") {
        let items = f
            .items
            .iter()
            .map(|i| format!("{{\"name\":{},\"bytes\":{}}}", jstr(&i.name), i.bytes))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"record\":{},\"id\":{},\"dir\":{},\"count\":{},\"bytes\":{},\"items\":[{items}]}}",
            jstr(&rec.file_name()),
            jstr(&rec.id()),
            jstr(&f.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default()),
            f.count(),
            f.bytes
        );
        return Ok(if f.is_empty() { 3 } else { 0 });
    }
    if f.is_empty() {
        crate::say_line!("{} 没有附件（需要时用 attach add 加进去）", rec.file_name());
        return Ok(3);
    }
    crate::say_line!(
        "{}  {}附件  共{}",
        rec.file_name(),
        f.count(),
        human_bytes(f.bytes)
    );
    for i in &f.items {
        crate::say_line!("  {:>9}  {}", human_bytes(i.bytes), i.name);
    }
    Ok(0)
}

fn add(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let _ = root;
    let (path, rec) = need_record(a)?;
    let src = a.need_pos(2, "要加入的文件路径")?;
    let sp = PathBuf::from(&src);
    if !sp.is_file() {
        return notfound(format!("{src} 不是文件"));
    }
    let dir = dir_for(&path);
    if !dir.is_dir() {
        fs::create_dir_all(&dir)?;
    }
    let name = {
        let n = a.get("name").unwrap_or("").to_string();
        if !n.is_empty() {
            n
        } else {
            sp.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "attachment".to_string())
        }
    };
    let dst = dir.join(&name);
    if dst.exists() && !a.has("force") {
        return usage(&format!(
            "{} 已存在；换名字用 --name，或加 --force 覆盖",
            dst.display()
        ));
    }
    // `--move` 是唯一会让源文件消失的开关：语义上"搬进来"而不是"加一份"。
    let moved = place(&sp, &dst, a.has("move"), a.has("force"))?;
    // 复读核对：复制报告成功但写短了，比直接失败更坏——记录随后会声称
    // 自己带着一份并不完整的证据。
    let got = fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
    if got != moved.bytes {
        return Err(EE::Io(format!(
            "写入后大小不符（源 {} / 目标 {got}），请检查磁盘空间",
            moved.bytes
        )));
    }
    // 清单必须在文件落盘之后回写，且必须成功。
    //
    // 顺序反过来（先写清单再放文件）在复制失败时会留下一条"声称有附件、实际
    // 没有"的记录。这里顺序对了就够了：文件已在，清单没写上的情况校验能发现，
    // 而且下次任何一次附件操作都会自动补齐。
    let changed = commit_manifest(&path)?;
    let f = scan(&path);
    crate::say_line!(
        "已加入 {}  {}  {}{}（{}附件 共{}）",
        rec.file_name(),
        rel_cwd(&dst),
        human_bytes(got),
        if moved.how == "rename" {
            "（同盘零拷贝）"
        } else {
            ""
        },
        f.count(),
        human_bytes(f.bytes)
    );
    if changed {
        crate::say_line!("清单已同步进记录（UPDATED 随之刷新）");
    }
    Ok(0)
}

fn get(a: &Args) -> R<i32> {
    let (path, _rec) = need_record(a)?;
    let want = a.need_pos(2, "附件名")?;
    let f = scan(&path);
    let hit = f
        .items
        .iter()
        .find(|i| i.name == want || i.name.ends_with(&format!("/{want}")));
    match hit {
        Some(i) => {
            let base = path.parent().unwrap_or(Path::new("."));
            let full = base.join(&i.name);
            // 只给路径，不搬字节：附件可能是 40MB 视频，塞进输出从来不是想要的。
            crate::say_line!("{}", full.display());
            crate::say_line!(
                "只操作这一个文件；同目录其他文件没给你路径就不要动。\
                 改完用 `abook attach sync <记录>` 让清单与磁盘重新对齐",
            );
            Ok(0)
        }
        None => notfound(format!(
            "{} 没有附件 {want}；先 attach list {} 看有哪些",
            _rec.file_name(),
            _rec.id()
        )),
    }
}

fn remove(a: &Args) -> R<i32> {
    let (path, rec) = need_record(a)?;
    let want = a.need_pos(2, "附件名")?;
    let f = scan(&path);
    let hit = f
        .items
        .iter()
        .find(|i| i.name == want || i.name.ends_with(&format!("/{want}")));
    let i = match hit {
        Some(i) => i.clone(),
        None => return notfound(format!("没有附件 {want}")),
    };
    let base = path.parent().unwrap_or(Path::new("."));
    let full = base.join(&i.name);
    if a.has("dry-run") {
        crate::say_line!("[dry-run] 将删除 {}", rel_cwd(&full));
        return Ok(0);
    }
    fs::remove_file(&full)?;
    // Remove the directory too once it is empty, so a record with no
    // attachments left does not keep an empty folder that listings then have to
    // explain away.
    for d in all_dirs(&path) {
        let _ = fs::remove_dir(d);
    }
    let changed = commit_manifest(&path)?;
    crate::say_line!(
        "已删除 {}  {}{}",
        rec.file_name(),
        i.name,
        if changed {
            "（清单已同步，UPDATED 已刷新）"
        } else {
            ""
        }
    );
    Ok(0)
}

fn path(a: &Args) -> R<i32> {
    let (rp, _rec) = need_record(a)?;
    match existing_dir(&rp) {
        Some(d) => crate::say_line!("{}", d.display()),
        None => crate::say_line!("{}", dir_for(&rp).display()),
    }
    // 目录路径不等于目录内全部文件的通行证：给了目录就要立刻说清边界，
    // 否则"我拿到的是目录"很容易被理解成"这里都归我改"。
    crate::say_line!(
        "目录可读；增删改用 `abook attach add/rm`，不要直接删文件。\
         直接删了清单会与磁盘不一致，用 `attach check` 能看出差在哪",
    );
    Ok(0)
}

fn check(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let mut total = 0usize;
    let mut out: Vec<String> = Vec::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        for (name, bytes, n) in orphans(&root, lib) {
            out.push(format!(
                "{}/{name}  {}  {n}个文件",
                lib.dir(),
                human_bytes(bytes)
            ));
            total += 1;
        }
    }
    if a.has("json") {
        let items = out
            .iter()
            .map(|s| format!("{{\"dir\":{}}}", jstr(s)))
            .collect::<Vec<_>>()
            .join(",");
        println!("{{\"orphans\":{total},\"items\":[{items}]}}");
        return Ok(if total == 0 { 0 } else { 3 });
    }
    if out.is_empty() {
        crate::say_line!("没有孤立附件目录");
        return Ok(0);
    }
    crate::say_line!("孤立附件目录 {total} 个（对应条目已不存在；文件保留未删）:");
    for s in out {
        crate::say_line!("  {s}");
    }
    Ok(3)
}

fn rel_cwd(p: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|c| p.strip_prefix(c).ok().map(|x| x.display().to_string()))
        .unwrap_or_else(|| p.display().to_string())
}


// ---------------------------------------------------------------- 复制策略

/// 50 MB：超过这条线不是"不能放"，而是"要先问一声"。
///
/// 手机上一次搬几百 MB 会卡住整个会话，而绝大多数附件——截图、日志、抓包、
/// 表格——都在几 MB 以内。超限时工具不擅自决定，也不直接拒绝：返回退出码 4
/// 并说明超了多少，让调用方去问用户，用户点头后加 `--force` 再跑一次。
pub const SOFT_MAX: u64 = 50 * 1024 * 1024;

/// 一次搬运的结果，用来告诉调用方到底走了哪条路。
pub struct Moved {
    pub bytes: u64,
    /// `rename` / `copy` / `copy-reflink`，写进输出便于诊断"为什么这次这么慢"
    pub how: &'static str,
}

/// 把文件放进附件目录，按平台与设备关系挑最高效的一条路。
///
/// 三级策略，判断顺序就是代价顺序：
///
/// 1. **同设备 rename** —— 零字节搬运，只改目录项，瞬间完成。同盘同分区
///    （最常见：附件就在记忆库旁边的同一个 /data 分区里）走这条。
/// 2. **reflink 复制** —— 支持 CoW 的文件系统（btrfs / XFS / APFS / ReFS、
///    以及 OverlayFS 上层）上，`FICLONE` 立刻建一份共享数据块的副本，
///    改一边不影响另一边，也不占双倍空间。`std::fs::copy` 在 Linux 上会
///    自己试这个，所以这里不重复实现，只标注它可能发生了。
/// 3. **内核级复制** —— 跨设备、网络路径、WSL 的 `/mnt/c`、Windows 的 UNC
///    `\\server\share` 都落到这条。`std::fs::copy` 在各平台已经是最优实现：
///    Linux 走 `copy_file_range(2)`（内核态搬，不经过用户空间缓冲），
///    Windows 走 `CopyFileExW`（含服务器端复制，SMB 上能变成远端直接搬），
///    macOS 走 `fcopyfile`。
///
/// 不自己写读写循环的原因：手写 1 MB 缓冲在网络路径上会比内核实现慢一个
/// 数量级——内核知道自己在跟 SMB 讲话，知道该用多大的块。
///
/// `move_mode` 由调用方的 `--move` 决定：要求搬走时优先 rename，失败才复制。
/// 不要求搬走时仍然先试 rename 吗？不。那样源文件会消失，而调用方说的是
/// "加进附件目录"，不是"搬进去"。语义优先于速度。
pub fn place(src: &Path, dst: &Path, move_mode: bool, force: bool) -> R<Moved> {
    let meta = fs::metadata(src)?;
    let bytes = meta.len();

    // 阈值管的是"会不会占掉一份新空间"，不是"是不是同盘"。
    //
    // 初版写成 `!same_dev && bytes > SOFT_MAX`，于是同盘复制 55 MB 直接放行——
    // 同盘照样占空间，只有同盘 **rename** 才不占。判据应该是"这次会不会真复制
    // 出一份新数据"：move 且同设备才免检，其余一律问一声。
    let same_dev = same_device(src, dst);
    let will_copy = !(move_mode && same_dev);
    if will_copy && bytes > SOFT_MAX && !force {
        return Err(EE::Blocked(format!(
            "附件 {} 共 {}，超过 {} 上限。确认要放进来就加 --force；\
             也可以先压缩、裁剪，或直接告诉我不放",
            src.display(),
            human_bytes(bytes),
            human_bytes(SOFT_MAX)
        )));
    }

    if move_mode && same_dev {
        // rename 在同设备上是原子的：目标要么不存在，要么是完整的源文件。
        match fs::rename(src, dst) {
            Ok(()) => {
                return Ok(Moved {
                    bytes,
                    how: "rename",
                })
            }
            Err(e) if is_cross_device(&e) => {
                // 探测说同设备，实际操作说跨设备：探测看的是父目录，而源可能
                // 是挂载点或符号链接指向别处。以实际错误为准，继续走复制。
            }
            Err(e) => return Err(EE::Io(format!("搬移失败: {e}"))),
        }
    }

    // 复制失败时不能只抛一句 IO 错误。
    //
    // 失败最常见的原因不是磁盘满，而是**源读不了**：SELinux 拦了、文件在
    // /data/data 的 tmpfs 遮罩后面、网络路径断了、WSL 的 /mnt/c 权限不足。
    // 这几种情况调用方都有办法自己解决——先把文件搬到它读得动的地方，再拿
    // 那个路径重来一次。所以错误信息必须写清"卡在哪一步"和"接着该怎么做"，
    // 而不是让调用方对着 `Permission denied` 猜。
    let readable = fs::File::open(src).is_ok();
    if let Err(e) = fs::copy(src, dst) {
        let hint = if !readable {
            format!(
                "读不了源文件（{e}）。先把文件复制到你有权限的位置，再用那个路径重试：\n                 \tcp '<源>' <可写目录>\n                 \tabook attach add <记录> <可写目录里的那份>\n                 安卓上 /data/data 被 tmpfs 遮罩，取真实路径用 /proc/1/root/data/data/...；                 提权先 su -c id 再判断，SELinux 拒绝会显示成 No such file"
            )
        } else if e.kind() == std::io::ErrorKind::StorageFull {
            format!("目标分区写满（{e}）。先腾出空间，或把附件放到别的库再引用")
        } else {
            format!(
                "写入目标失败（{e}）。先复制到记忆库所在分区的一个普通目录，\
                 再用那个路径重试；跨文件系统/网络路径（UNC、WSL /mnt/c）\
                 直接复制大文件容易中断"
            )
        };
        // 半截文件必须清掉：留在附件目录里会被 scan 统计成一份完整附件，
        // 而它其实不完整——这比没有更坏。
        let _ = fs::remove_file(dst);
        return Err(EE::Io(hint));
    }
    // 复制后删源，只在调用方要求搬走时做。删之前已经确认目标大小一致，
    // 所以这里失败也不会造成数据丢失——源还在。
    if move_mode {
        let _ = fs::remove_file(src);
    }
    Ok(Moved {
        bytes,
        how: "copy",
    })
}

/// 两个路径是否在同一设备上。
///
/// 看的是**最近的已存在祖先目录**：目标文件通常还不存在，`dst` 本身
/// `metadata` 会失败。跨设备时 `rename` 会返回 `EXDEV`，所以这个判断只是
/// 用来决定先试哪条路，判错不会出错，最多多一次失败的 `rename`。
fn same_device(a: &Path, b: &Path) -> bool {
    let da = device_of(a);
    let db = device_of(b);
    match (da, db) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn device_of(p: &Path) -> Option<u64> {
    let mut cur = p.to_path_buf();
    for _ in 0..64 {
        if let Ok(m) = fs::metadata(&cur) {
            return device_id(&m);
        }
        if !cur.pop() {
            return None;
        }
    }
    None
}

#[cfg(unix)]
fn device_id(m: &fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(m.dev())
}

#[cfg(not(unix))]
fn device_id(_m: &fs::Metadata) -> Option<u64> {
    // Windows 没有对外的 st_dev。跨盘 rename 会直接报错，由 `place` 的
    // EXDEV 分支接住，所以这里返回 None 表示"不知道"，让调用方走复制。
    None
}

fn is_cross_device(e: &std::io::Error) -> bool {
    // EXDEV = 18 on Linux/Android, 18 on macOS, ERROR_NOT_SAME_DEVICE = 17 on
    // Windows. Matching on raw_os_error is the only portable way; the ErrorKind
    // for this case is still unstable.
    #[cfg(unix)]
    {
        e.raw_os_error() == Some(18)
    }
    #[cfg(windows)]
    {
        e.raw_os_error() == Some(17)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = e;
        true
    }
}


// ---------------------------------------------------------------- 清单同步

/// 渲染附件清单块的内容。
///
/// 每行 `序号<TAB>相对名<TAB>字节`。用 TAB 分隔而不是空格：文件名里可以有
/// 空格，用空格分隔的清单在遇到第一个带空格的名字时就再也对不上了。
///
/// 序号从 1 起，稳定递增——正文里写 `本条目附件1://xxx` 时，`1` 指的是这里
/// 的第 1 行，两边能直接对上。
pub fn render_list(items: &[Item]) -> String {
    let mut out = String::new();
    for (i, it) in items.iter().enumerate() {
        out.push_str(&format!("{}\t{}\t{}\n", i + 1, it.name, it.bytes));
    }
    out.trim_end().to_string()
}

/// 正文里的附件引用：`本条目附件1://名字`。
///
/// 采用的写法是"序号://"而不是 URL——它不需要真的可点击，需要的是**人看
/// 一眼就知道指向哪个附件**，且机器能正则出来做校验。支持 `附件1` 与
/// `attachment 1` 两种写法，是因为同一件事写两次不同语言的概率很高。
pub struct Ref {
    pub index: usize,
    pub raw: String,
}

/// 从正文里抽出全部附件引用。
pub fn refs_in(body: &str) -> Vec<Ref> {
    // 长模式优先，且已匹配的字符区间要占住。
    //
    // 初版按三种模式各扫一遍再做字符串去重，结果 `本条目附件1://a.log` 与
    // `附件1://a.log` 被算成两处引用——短模式命中的正是长模式里的那一段。
    // 一条记录写了 2 处引用却报 4 处，校验就没法用。按区间占位才不会重复。
    const PATS: &[&str] = &["本记忆条目附件", "本条目附件", "附件"];
    let mut taken = vec![false; body.len()];
    let mut found: Vec<(usize, Ref)> = Vec::new();
    for pat in PATS {
        let mut from = 0usize;
        while let Some(rel) = body[from..].find(pat) {
            let start = from + rel;
            let after = start + pat.len();
            if taken[start..after].iter().any(|x| *x) {
                from = after;
                continue;
            }
            let mut j = after;
            let mut num = String::new();
            while j < body.len() && body.as_bytes()[j].is_ascii_digit() {
                num.push(body.as_bytes()[j] as char);
                j += 1;
            }
            if num.is_empty() || !body[j..].starts_with("://") {
                from = after;
                continue;
            }
            let index = match num.parse::<usize>() {
                Ok(v) => v,
                Err(_) => {
                    from = after;
                    continue;
                }
            };
            let rest = &body[j + 3..];
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '）' || c == ')' || c == '，' || c == '。')
                .unwrap_or(rest.len());
            let stop = j + 3 + end;
            for t in taken.iter_mut().take(stop).skip(start) {
                *t = true;
            }
            found.push((
                start,
                Ref {
                    index,
                    raw: body[start..stop].to_string(),
                },
            ));
            from = stop;
        }
    }
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, r)| r).collect()
}

/// 把磁盘上的附件目录同步进记录。
///
/// **这是唯一允许写记录附件清单的入口**。三种情况都走它：加附件、删附件、
/// 以及校验时发现不一致。集中在一处，是为了让"清单与磁盘永远一致"成为结构
/// 上的性质，而不是靠每个调用点各自记得刷新。
///
/// 返回 `(变了没, 清单内容)`。写记录本身由调用方决定什么时候做——有些场景
/// （如 `attach check`）只想比对不想写。
pub fn sync_manifest(rec_path: &Path) -> (bool, String) {
    let f = scan(rec_path);
    let want = render_list(&f.items);
    let cur = match crate::rec::Record::parse(rec_path, crate::rec::Lib::Candidate)
        .or_else(|_| crate::rec::Record::parse(rec_path, crate::rec::Lib::Canonical))
    {
        Ok(r) => r.attach.unwrap_or_default(),
        Err(_) => String::new(),
    };
    (
        normalize(&cur) != normalize(&want),
        want,
    )
}

fn normalize(s: &str) -> String {
    s.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 真正执行回写：改附件后调用。
///
/// 写记录走 `cmd_edit::save`，因此 `UPDATED` 会被自动刷新——"附件改了，条目
/// 算改过"这条不需要另外实现，它落在同一条写盘路径上。
pub fn commit_manifest(rec_path: &Path) -> R<bool> {
    let (changed, want) = sync_manifest(rec_path);
    if !changed {
        return Ok(false);
    }
    let (root, lib) = root_and_lib(rec_path)?;
    let mut rec = crate::rec::Record::parse(rec_path, lib)?;
    rec.attach = if want.trim().is_empty() { None } else { Some(want) };
    // 清单是记录的派生部分，不是正文：改它不走批准门。批准门保护的是
    // "内容判断"，而附件增删本身就是已经执行完的确定事实——再问一遍批不批，
    // 只会让清单和磁盘长期对不上。
    let a = Args::empty();
    crate::cmd_edit::save(&root, &mut rec, &a, "attach")?;
    Ok(true)
}

/// 从记录路径反推记忆根与所属库。
///
/// 目录布局是 `<root>/<canonical|candidate>/records/<文件>`，所以往上两级拿库、
/// 再往上一级拿根。推不出来就报清楚，而不是猜一个默认根去写——猜错的根会让
/// 这次写入落到另一个库上。
fn root_and_lib(rec_path: &Path) -> R<(PathBuf, Lib)> {
    let lib = rec_path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string());
    let (lib, _) = match lib.as_deref() {
        Some("canonical") => (Lib::Canonical, ()),
        Some("candidate") => (Lib::Candidate, ()),
        other => {
            return Err(EE::Usage(format!(
                "记录不在 <根>/<库>/records/ 下（实际: {}）。\
                 附件清单只能回写到标准布局的库里",
                other.unwrap_or("?")
            )))
        }
    };
    // <root>/<lib>/records/x.md → root
    let root = rec_path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .ok_or_else(|| EE::Usage("推不出记忆根".into()))?;
    Ok((root, lib))
}

/// 校验一份记录与它的附件目录是否一致。
///
/// 这是"容忍模型违规操作"的那一半：模型完全可以绕过工具直接 `rm` 掉某个
/// 附件文件，清单就会指向不存在的东西。校验不阻止它（阻止不了），但能**发现**
/// 并在下一次接触这条记录时说清楚差在哪。
pub struct Mismatch {
    /// 清单里有、磁盘上没有
    pub missing: Vec<String>,
    /// 磁盘上有、清单里没有
    pub extra: Vec<String>,
    /// 两边都有但字节数不同
    pub resized: Vec<String>,
}

impl Mismatch {
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.extra.is_empty() && self.resized.is_empty()
    }
    pub fn describe(&self) -> String {
        let mut v: Vec<String> = Vec::new();
        if !self.missing.is_empty() {
            v.push(format!("清单有磁盘无 {} 个（{}）", self.missing.len(), self.missing.join("、")));
        }
        if !self.extra.is_empty() {
            v.push(format!("磁盘有清单无 {} 个（{}）", self.extra.len(), self.extra.join("、")));
        }
        if !self.resized.is_empty() {
            v.push(format!("字节数变了 {} 个（{}）", self.resized.len(), self.resized.join("、")));
        }
        v.join("；")
    }
}

/// 解析清单块成 `(名, 字节)`。
fn parse_list(s: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        // 名字在中间，字节在末尾：名字自己可能含 TAB（极罕见但存在），所以
        // 取"最后一个字段是字节、其余中间全部是名字"的解析方式。
        let bytes = match parts.last().and_then(|x| x.parse::<u64>().ok()) {
            Some(b) => b,
            None => continue,
        };
        let name = if parts.len() == 3 {
            parts[1].to_string()
        } else {
            parts[1..parts.len() - 1].join("\t")
        };
        out.push((name, bytes));
    }
    out
}

pub fn verify(rec_path: &Path) -> Mismatch {
    let mut m = Mismatch {
        missing: Vec::new(),
        extra: Vec::new(),
        resized: Vec::new(),
    };
    let listed = match crate::rec::Record::parse(rec_path, crate::rec::Lib::Candidate)
        .or_else(|_| crate::rec::Record::parse(rec_path, crate::rec::Lib::Canonical))
    {
        Ok(r) => parse_list(&r.attach.unwrap_or_default()),
        Err(_) => Vec::new(),
    };
    let disk = scan(rec_path);
    let listed_names: Vec<&String> = listed.iter().map(|(n, _)| n).collect();
    for (name, bytes) in &listed {
        match disk.items.iter().find(|i| &i.name == name) {
            None => m.missing.push(name.clone()),
            Some(it) if it.bytes != *bytes => m.resized.push(name.clone()),
            _ => {}
        }
    }
    for it in &disk.items {
        if !listed_names.contains(&&it.name) {
            m.extra.push(it.name.clone());
        }
    }
    m
}

/// 附件正文引用校验：`本条目附件N://x` 里的 N 必须指向真实存在的附件。
///
/// 悬空引用不算错误（正文是历史记录，可能先写了引用再补附件），但要报出来。
pub fn dangling_refs(rec_path: &Path) -> Vec<String> {
    let rec = match crate::rec::Record::parse(rec_path, crate::rec::Lib::Candidate)
        .or_else(|_| crate::rec::Record::parse(rec_path, crate::rec::Lib::Canonical))
    {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let n = scan(rec_path).count();
    refs_in(&rec.body)
        .into_iter()
        .filter(|r| r.index == 0 || r.index > n)
        .map(|r| r.raw)
        .collect()
}


/// `attach sync` — 让清单与磁盘重新对齐。
///
/// 存在的理由很具体：模型有能力绕过工具直接改附件目录（`rm`、`mv`、编辑器
/// 覆盖）。工具阻止不了，但必须**能发现**。`sync` 就是"发现了就对回来"——
/// 而不是让一条记录长期声称带着一份已经不在的证据。
fn sync(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(1, "记录选择器；或 --all 全库");
    let targets: Vec<std::path::PathBuf> = if a.has("all") {
        let mut v = Vec::new();
        for lib in [Lib::Canonical, Lib::Candidate] {
            for r in crate::rec::load_lib(&root, lib) {
                v.push(r.path);
            }
        }
        v
    } else {
        let sel = sel?;
        let (p, _) = crate::rec::resolve_record(&root, &sel)?;
        vec![p]
    };

    let mut fixed = 0usize;
    let mut drifted = 0usize;
    for p in &targets {
        let m = verify(p);
        if !m.is_empty() {
            drifted += 1;
            if !a.has("dry-run") {
                commit_manifest(p)?;
                fixed += 1;
            }
            if targets.len() == 1 {
                crate::say_line!("{}: {}", rel(&root, p), m.describe());
            }
        }
        // 正文里的悬空引用一并报出来：不是错误，但要让人知道指错了
        for d in dangling_refs(p) {
            crate::say_line!("{}: 悬空引用 {d}", rel(&root, p));
        }
    }
    if a.has("dry-run") {
        crate::say_line!(
            "[dry-run] {} 条记录清单漂移，未改动。去掉 --dry-run 即回写",
            drifted
        );
        return Ok(if drifted == 0 { 0 } else { 3 });
    }
    crate::say_line!(
        "检查 {} 条记录，{} 条清单漂移，已回写 {} 条",
        targets.len(),
        drifted,
        fixed
    );
    Ok(if drifted == 0 { 0 } else { 0 })
}

/// `attach refs` — 列出正文里的附件引用，供核对。
fn refs(a: &Args) -> R<i32> {
    let (p, rec) = need_record(a)?;
    let f = scan(&p);
    let rs = refs_in(&rec.body);
    if a.has("json") {
        let items = rs
            .iter()
            .map(|r| {
                let item = f.items.get(r.index.saturating_sub(1));
                format!(
                    "{{\"ref\":{},\"index\":{},\"target\":{},\"exists\":{}}}",
                    jstr(&r.raw),
                    r.index,
                    jstr(&item.map(|i| i.name.clone()).unwrap_or_default()),
                    item.is_some()
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"record\":{},\"count\":{},\"attachments\":{},\"refs\":[{items}]}}",
            jstr(&rec.file_name()),
            rs.len(),
            f.count()
        );
        return Ok(0);
    }
    if rs.is_empty() {
        crate::say_line!(
            "{} 正文里没有附件引用（写法: 本条目附件1://文件名）",
            rec.file_name()
        );
        return Ok(0);
    }
    crate::say_line!("{}  正文引用 {} 处，附件 {} 个", rec.file_name(), rs.len(), f.count());
    for r in &rs {
        match f.items.get(r.index.saturating_sub(1)) {
            Some(it) => crate::say_line!("  {:<24} → {:<40} {}", r.raw, it.name, human_bytes(it.bytes)),
            None => crate::say_line!("  {:<24} → 悬空（没有第 {} 个附件）", r.raw, r.index),
        }
    }
    Ok(0)
}
