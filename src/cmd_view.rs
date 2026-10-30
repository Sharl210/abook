//! Viewing: paged listings, single-field read, history, diff, export, tree.

use crate::paging::Page;
use crate::rec::{load_lib, resolve_record, ListFilter, Lib, Record};
use crate::util::*;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- 列表元信息
//
// 放在这里而不是 `util.rs`：`util.rs` 正被另一个人改，而且这一组函数的语义
// 属于"列表怎么显示"，与工具函数不同层。

/// 行数算法的唯一实现。
///
/// 此前两种写法并存：`index`/`branch`/`skill` 数 `\n` 的个数，其余用
/// `lines().count()`。两者在"文件末尾没有换行"时差 1——工具自己写的记录一定以
/// 换行结尾，但库允许手改，于是同一条记录在 `list` 里 12 行、在 `index` 里 11 行。
/// 按逻辑行数算：末尾不带换行也算最后一行，空文件 0 行。
///
/// 返回 `None` 表示这不是文本（含 NUL 字节），二进制没有"行"这个概念——不要拿
/// 一个数字冒充。收 `Read` 是为了让几十 MB 的附件也能数行而不必整个读进内存。
pub fn count_lines_from<R: std::io::Read>(mut r: R) -> Option<usize> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut newlines = 0usize;
    let mut total = 0u64;
    let mut last: Option<u8> = None;
    // 跨块被切断的多字节字符尾巴，最多 3 字节。
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let k = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => k,
            Err(_) => return None,
        };
        if k == 0 {
            continue;
        }
        if buf[..k].contains(&0) {
            // NUL 在 UTF-8 里是合法字符，实际出现只说明这是二进制或 UTF-16。
            return None;
        }
        pending.extend_from_slice(&buf[..k]);
        let advance = match std::str::from_utf8(&pending) {
            Ok(s) => {
                newlines += s.bytes().filter(|c| *c == b'\n').count();
                let n = pending.len();
                total += n as u64;
                last = pending.last().copied();
                pending.clear();
                n
            }
            Err(e) => {
                // `error_len() == None` 表示"这一块结尾被切断了"，不是非法字节：
                // 留下尾巴等下一块。
                if e.error_len().is_some() {
                    return None;
                }
                let valid = e.valid_up_to();
                let s = std::str::from_utf8(&pending[..valid]).ok()?;
                newlines += s.bytes().filter(|c| *c == b'\n').count();
                total += valid as u64;
                if valid > 0 {
                    last = Some(pending[valid - 1]);
                }
                pending.drain(..valid);
                valid
            }
        };
        let _ = advance;
    }
    // 结尾残留不完整的多字节序列：这不是文本。
    if !pending.is_empty() {
        return None;
    }
    if total == 0 {
        return Some(0);
    }
    Some(if last == Some(b'\n') { newlines } else { newlines + 1 })
}

/// 记录正文的行数。记录一定是文本，所以返 0 只可能是读失败——那种情况调用方
/// 应该用 [`count_lines_of_file`] 拿到 `None`。
pub fn count_lines(s: &str) -> usize {
    count_lines_from(s.as_bytes()).unwrap_or(0)
}

/// 同上，直接吃字节：`index`/`branch` 只读字节、不解析记录。
pub fn count_lines_bytes(b: &[u8]) -> usize {
    count_lines_from(b).unwrap_or(0)
}

/// 文本文件的行数；二进制或打不开都给 `None`。
pub fn count_lines_of_file(p: &Path) -> Option<usize> {
    count_lines_from(std::fs::File::open(p).ok()?)
}

/// 时间列的文本写法，与 `util::row_meta` 同一口径：未知给 `-`。
///
/// `row_meta` 把这条判断写在函数里，JSON 侧没有对应物，于是同一条读不出时间的
/// 记录在文本里是 `-`、在 JSON 里是 1970 年。
pub fn when_or_dash(ts: i64) -> String {
    if ts > 0 {
        crate::util::short_when(ts)
    } else {
        "-".to_string()
    }
}

/// JSON 的未知维度写 `null`，不写 0。
pub fn jopt<T: std::fmt::Display>(v: Option<T>) -> String {
    match v {
        Some(x) => x.to_string(),
        None => "null".to_string(),
    }
}

/// 列表行元信息的 JSON 版，与 `row_meta` 一一对应。
///
/// 文本打 `-` 的维度这里必须是 `null`：`index --json` 曾经把读不出来的记录报成
/// `"bytes":0,"lines":0,"updated":0`，机器调用方看到的是"0 字节 0 行、更新于
/// 1970"，而文本侧说的是"元信息不可得"。同一份数据两套说法，机器侧只会更相信
/// 那个数字。
pub fn meta_json(when: i64, bytes: Option<u64>, lines: Option<usize>) -> String {
    format!(
        "\"bytes\":{},\"lines\":{},\"updated\":{},\"when\":{}",
        jopt(bytes),
        jopt(lines),
        if when > 0 {
            when.to_string()
        } else {
            "null".to_string()
        },
        jstr(&when_or_dash(when))
    )
}

/// `get <record> <field>` — read one field without dumping the whole record.
pub fn cmd_get(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sel = a.need_pos(0, "记录")?;
    let (path, lib) = resolve_record(&root, sel)?;
    let rec = Record::parse(&path, lib)?;
    let want_json = a.has("json");
    // `--field` was listed in the parameter table but the code only ever read
    // the second positional, so `get X --field TITLE` printed every field name
    // instead of the title. Both spellings are honoured now.
    let field_arg: Option<String> = a
        .get("field")
        .map(|s| s.to_string())
        .or_else(|| a.pos.get(1).cloned());
    match field_arg.as_ref() {
        Some(f) => {
            let (canon, _) = crate::parse::canon_key(f);
            if canon == "BODY" || canon == "MEMORY_BODY" {
                if want_json {
                    println!(
                        "{{\"id\":{},\"path\":{},\"field\":\"BODY\",\"value\":{}}}",
                        jstr(&rec.id()),
                        jstr(&rel(&root, &path)),
                        jstr(&rec.body)
                    );
                } else {
                    println!("{}", rec.body);
                }
                return Ok(0);
            }
            // `UPDATED`/`CREATED` are answerable even on records that predate
            // the field, because they are derived. Without this branch a caller
            // asking for the date on an old record got "记录没有字段", which
            // reads as a broken record rather than an old one.
            if canon == "UPDATED" || canon == "CREATED" {
                let v = if canon == "UPDATED" {
                    crate::util::short_when(rec.updated())
                } else {
                    crate::util::short_when(rec.created())
                };
                if want_json {
                    println!(
                        "{{\"id\":{},\"path\":{},\"field\":{},\"value\":{}}}",
                        jstr(&rec.id()),
                        jstr(&rel(&root, &path)),
                        jstr(&canon),
                        jstr(&v)
                    );
                } else {
                    println!("{v}");
                }
                return Ok(0);
            }
            if !rec.has(&canon) {
                // The field genuinely is absent. Say which record and which
                // fields it does have -- a bare "not found" leaves the caller
                // guessing whether the record or the field name was wrong.
                return notfound(format!(
                    "记录没有字段 {canon}（{} 共 {} 个字段；时间用 UPDATED/CREATED 查询）",
                    rec.id(),
                    rec.fields.len()
                ));
            }
            if want_json {
                println!(
                    "{{\"id\":{},\"path\":{},\"field\":{},\"value\":{}}}",
                    jstr(&rec.id()),
                    jstr(&rel(&root, &path)),
                    jstr(&canon),
                    jstr(&rec.get(&canon))
                );
            } else {
                println!("{}", rec.get(&canon));
            }
            Ok(0)
        }
        None => {
            if want_json {
                let mut items: Vec<String> = Vec::new();
                for (k, v) in &rec.fields {
                    items.push(format!(
                        "{{\"field\":{},\"chars\":{},\"preview\":{}}}",
                        jstr(k),
                        v.chars().count(),
                        jstr(&truncate_chars(v, 60))
                    ));
                }
                for (k, v) in rec.promo_fields() {
                    items.push(format!(
                        "{{\"field\":{},\"chars\":{},\"preview\":{}}}",
                        jstr(&k),
                        v.chars().count(),
                        jstr(&truncate_chars(&v, 60))
                    ));
                }
                items.push(format!(
                    "{{\"field\":\"BODY\",\"chars\":{},\"preview\":{}}}",
                    rec.body.chars().count(),
                    jstr(&truncate_chars(&rec.body, 60))
                ));
                let ver = crate::util::file_version(&path);
                println!(
                    "{{\"id\":{},\"path\":{},\"created\":{},\"updated\":{},\"when\":{},\"version\":{},\"fields\":[{}]}}",
                    jstr(&rec.id()),
                    jstr(&rel(&root, &path)),
                    rec.created(),
                    rec.updated(),
                    jstr(&crate::util::short_when(rec.updated())),
                    jstr(&ver),
                    items.join(",")
                );
                return Ok(0);
            }
            // no field: list every readable field, tail blocks included
            //
            // The two time values appear as pseudo-fields so they line up with
            // the real ones: a caller that greps for `UPDATED` finds a row in
            // either shape, and a record that predates the field still answers
            // instead of looking broken.
            //
            // VERSION 同样是伪字段，但它是**写保护**用的：这是本条记录当前
            // 内容的指纹，读的时候拿上它，改的时候用 `--if-version` 带上。
            //
            // 为什么必须由读的人自己拿：命令内部的"写前重新检查一遍"永远
            // 通过——它看到的就是当前字节。真正的危险窗口在两次命令之间：
            // 读完、思考几分钟、再写，中间别人可能已经改过。只有跨过这个
            // 窗口带下来的指纹才能发现分叉。所以指纹必须出现在读取输出里，
            // 否则 `--if-version` 这个功能等于没法用。
            println!(
                "UPDATED\t{}\t{}",
                crate::util::short_when(rec.updated()).chars().count(),
                crate::util::short_when(rec.updated())
            );
            println!(
                "CREATED\t{}\t{}",
                crate::util::short_when(rec.created()).chars().count(),
                crate::util::short_when(rec.created())
            );
            // Skip the stored UPDATED/CREATED rows: they were already printed
            // above from the same source. Printing both made one field appear
            // twice with different lengths (11 versus 40 characters), so a caller
            // scraping this output could not tell which value was authoritative.
            // A record that has no stored field still gets the row from the two
            // pseudo-lines above, which is why those exist.
            for (k, v) in &rec.fields {
                if k == "UPDATED" || k == "CREATED" {
                    continue;
                }
                println!("{k}\t{}\t{}", v.chars().count(), truncate_chars(v, 60));
            }
            for (k, v) in rec.promo_fields() {
                println!("{k}\t{}\t{}", v.chars().count(), truncate_chars(&v, 60));
            }
            println!("BODY\t{}\t{}", rec.body.chars().count(), truncate_chars(&rec.body, 60));
            // 指纹必须出现在读取输出里，否则 --if-version 没法用。
            let fp = crate::util::file_version(&path);
            if !fp.is_empty() {
                println!("VERSION\t{}\t{}", fp.chars().count(), fp);
            }
            Ok(0)
        }
    }
}

/// Shared lister for the library's note directories.
///
/// `commits`/`review`/`staging` hold real files with real content — the live
/// library has 107 commits, 21 review entries and 1 staging item — but only
/// `commits` was readable (through `history`). `paths` printed the other two
/// as though they were part of the tool's vocabulary while nothing could open
/// them, so an agent knew they existed and still could not read them. One
/// implementation here means adding a future directory is a single line.
fn note_dir(
    a: &Args,
    dir: &str,
    noun: &str,
    json_key: &str,
) -> R<i32> {
    let root = resolve_root(a);
    let d = root.join(dir);
    let needle = a.pos.first().cloned().unwrap_or_default();
    // `--show` opens one file whole; the positional argument filters the list.
    if let Some(name) = a.get("show") {
        let p = if Path::new(name).is_absolute() {
            PathBuf::from(name)
        } else {
            d.join(name)
        };
        // `staging resolve` 把结清件移进 `closed/` 之后，只敲文件名就再也读不出来
        // 了——文件明明还在库里，工具却说没有。归档不是删除，归档件必须仍可读，
        // 否则"当初是怎么定的"这句话就没有落地的地方。先在原地找，再在 closed/
        // 里找；两边都没有才报未找到。只有 `staging` 有归档子目录，别的目录（如
        // `review`）不该在提示里凭空提到一个它没有的地方。
        let archive = d.join("closed");
        let p = if p.is_file() {
            p
        } else {
            let alt = archive.join(name);
            if alt.is_file() {
                alt
            } else {
                // 未找到用 NotFound（退码 3）而不是 usage：`--show 打错名字` 不是
                // 命令用错了，是查的东西不在，调用方要能按退码分辨这两种情况。
                let also = if archive.is_dir() { "（含 closed/）" } else { "" };
                return Err(EE::NotFound(format!(
                    "{dir} 里没有 `{name}`{also}；先运行 `abook {dir}` 看有哪些"
                )));
            }
        };
        let txt = read_str(&p)?;
        if a.has("json") {
            println!(
                "{{\"name\":{},\"path\":{},\"bytes\":{},\"body\":{}}}",
                jstr(name),
                jstr(&p.display().to_string()),
                txt.len(),
                jstr(&txt)
            );
        } else {
            print!("{txt}");
            if !txt.ends_with('\n') {
                println!();
            }
        }
        return Ok(0);
    }
    // Carry each note's own time so the list can be ordered by recency.
    let mut rows: Vec<(String, String, u64, i64, usize)> = Vec::new();
    // Subdirectories are listed as rows, not skipped. `review/` really does
    // contain `abandoned-20260928/`, and silently dropping it made the count
    // disagree with `ls` while looking perfectly healthy -- the same
    // disappearance bug this tool exists to stop.
    let mut subdirs: Vec<(String, usize)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&d) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let n = std::fs::read_dir(&p).map(|r| r.flatten().count()).unwrap_or(0);
                let name = p
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                subdirs.push((name, n));
                continue;
            }
            if !p.is_file() {
                continue;
            }
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let txt = read_str(&p).unwrap_or_default();
            if !needle.is_empty() && !txt.contains(&needle) && !name.contains(&needle) {
                continue;
            }
            let title = txt
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim_start_matches('#')
                .trim()
                .to_string();
            let stamp = commit_time(&p, &txt);
            // 行数与体积、时间一起构成列表行的固定三件套：体积回答"要不要读",
            // 行数回答"读起来有多长",时间回答"还新不新"。少了行数这一列,
            // 读者只能按体积猜篇幅,而中文字节数与原行数不成比例。
            rows.push((name, title, txt.len() as u64, stamp, count_lines(&txt)));
        }
    }
    // Newest first. Sorting bare names put the old `0001-...` sequence after
    // every timestamped name and ordered `-1` before the bare name, so nearly
    // half a real listing landed in the wrong place. Each note records its own
    // time; ask the file.
    rows.sort_by(|x, y| y.3.cmp(&x.3).then_with(|| y.0.cmp(&x.0)));
    let pg = Page::from_args(a, rows.len());
    if a.has("json") {
        let items = rows
            .iter()
            .skip(pg.start())
            .take(pg.per.min(rows.len()))
            .map(|(n, t, b, ts, ln)| {
                format!(
                    "{{\"name\":{},\"title\":{},{}}}",
                    jstr(n),
                    jstr(t),
                    meta_json(*ts, Some(*b), Some(*ln))
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!("{{{},\"{json_key}\":[{items}]}}", pg.json_fields());
        return Ok(0);
    }
    for (n, t, b, ts, ln) in rows.iter().skip(pg.start()).take(pg.per.min(rows.len())) {
        println!(
            "{}  {n:<48}  {t}",
            crate::util::row_meta(*ts, Some(*b), Some(*ln))
        );
    }
    println!("{}", pg.bar());
    let sub = if subdirs.is_empty() {
        String::new()
    } else {
        let names = subdirs
            .iter()
            .map(|(n, c)| format!("{n}/（{c} 个文件）"))
            .collect::<Vec<_>>()
            .join("、");
        format!("；另有子目录 {names}，需直接查看")
    };
    println!("共 {} 条{noun}{sub}", rows.len());
    if !a.has("json") {
        println!("看全文: abook {dir} --show <文件名>");
    } else {
        // Machine callers must see the subdirectories too, or they will
        // conclude the directory holds fewer entries than it does.
        let s2 = subdirs
            .iter()
            .map(|(n, c)| format!("{{\"name\":{},\"files\":{c}}}", jstr(n)))
            .collect::<Vec<_>>()
            .join(",");
        eprintln!("{{\"subdirs\":[{s2}]}}");
    }
    Ok(0)
}

/// `staging` — pending decision items awaiting a human answer.
pub fn cmd_staging(a: &Args) -> R<i32> {
    if matches!(a.pos.first().map(|s| s.as_str()), Some("resolve") | Some("close")) {
        return staging_resolve(a);
    }
    note_dir(a, "staging", "暂存", "staging")
}

/// `staging resolve` — 结清一件待决暂存件。
///
/// 待决区此前只有列表和 `--show`，没有结清动作：回答完之后文件只能靠手工 `mv`
/// 才能离开，于是「待决」这个数字只增不减。一个长期不为零的待决数不再提示任何
/// 东西——它变成背景噪音，而真正需要人回答的那一件就淹在里面。缺这个动作的直接
/// 后果是：想清楚了也没有地方把决定落下，只能绕过工具改库。
///
/// 结清不是删除。原文一字不动保留，结论追加在末尾，整件移入 `closed/` 子目录：
/// 待决数回答「还有几件在等人」，`closed/` 回答「当初是怎么定的」。删掉原文等于
/// 删掉当时的真实认知，而认知会过期恰恰是当初把它暂存起来的原因。
pub fn staging_resolve(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let dir = root.join("staging");
    let closed = dir.join("closed");

    let raw = a.pos.get(1).cloned().unwrap_or_default();
    if raw.trim().is_empty() {
        return Err(EE::Usage(
            "用法: staging resolve <文件名> --decision \"结论\" [--ref <记录ID>] [--dry-run]".into(),
        ));
    }
    // 只接受 staging/ 下的一层文件名。`..` 或分隔符一旦放进来，这个命令就能把
    // 库外任意文件追加一段再删掉，而它的用途只是移一件待决件。
    if raw.contains('/') || raw.contains('\\') || raw == ".." || raw == "." {
        return Err(EE::Usage(format!(
            "{raw} 不是 staging/ 下的文件名；这个命令只处理待决区里的单件，不接受路径"
        )));
    }
    let name = if raw.ends_with(".md") { raw.clone() } else { format!("{raw}.md") };
    let src = dir.join(&name);
    let dst = closed.join(&name);

    // 幂等：已经结清过的件再结一次不是错误，是把当时的结论再读一遍。
    if !src.exists() {
        if dst.exists() {
            let prev = read_str(&dst).unwrap_or_default();
            let decided = prev
                .lines()
                .find(|l| l.trim_start().starts_with("- 决定："))
                .map(|l| l.trim_start().trim_start_matches("- 决定：").trim().to_string())
                .unwrap_or_default();
            if a.has("json") {
                println!(
                    "{{\"ok\":true,\"name\":{},\"already\":true,\"decision\":{}}}",
                    jstr(&name),
                    jstr(&decided)
                );
            } else {
                println!("已结清: {name}");
                if !decided.is_empty() {
                    println!("当时结论: {decided}");
                }
                println!("位置: {}", dst.display());
            }
            return Ok(0);
        }
        let live = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| n.ends_with(".md"))
                    .take(5)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let hint = if live.is_empty() {
            "；待决区当前为空".to_string()
        } else {
            format!("；待决区现有: {}", live.join("、"))
        };
        return Err(EE::NotFound(format!("staging/ 里没有 {name}{hint}")));
    }
    // 两边同时存在说明上一次结清只做了一半（新件写成、原件没删掉）。这时覆盖
    // 会丢掉 closed/ 里已经追加过的结论，所以停下来让人看一眼，而不是猜。
    if dst.exists() {
        return Err(EE::Integrity(format!(
            "staging/{name} 与 staging/closed/{name} 同时存在，上一次结清只完成了一半；\
先比对两份内容再决定留哪一份"
        )));
    }

    let decision = a.get("decision").map(|s| s.trim().to_string()).unwrap_or_default();
    if decision.is_empty() {
        return Err(EE::Usage(format!(
            "结清 {name} 要给出结论: --decision \"保持现状\" / \"批准入箱\" / 其它处置。\
没有结论的结清只是把问题藏进子目录"
        )));
    }
    // 结论是一条结论，不是一段正文：换行会让下面的 `- 决定：` 只剩第一行，读到
    // 第一行的人会以为决策就是那半句。
    let decision = decision.replace(['\n', '\r'], " ");
    let ref_id = a.get("ref").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

    let orig = read_str(&src)?;
    let st = stamps();
    let mut seg = String::new();
    // 两个换行：原文末尾的换行已被 trim_end 吃掉，只留一个的话 `## 结清` 会紧贴
    // 上一行，读起来像是原文的一部分，而不是后加的结论。
    seg.push_str("\n\n## 结清\n\n");
    seg.push_str(&format!("- 时间：{}\n", st.iso_local));
    seg.push_str(&format!("- 决定：{decision}\n"));
    if let Some(r) = &ref_id {
        seg.push_str(&format!("- 依据记录：{r}\n"));
    }
    seg.push_str("- 处置：本件移入 staging/closed/，不再计入待决数\n");
    let next = format!("{}{seg}", orig.trim_end());

    // 这里不能复用 `Args::dry_run()`：那条判据是「没有 --apply/--write/--yes 就当
    // 预演」，为的是让 `repair`/`normalize` 这类批量写入默认安全。结清一件待决件
    // 本来就不该要求 `--apply`——它只有一个动作、只动一个文件、而且可逆（原文一字
    // 不动搬进 closed/）。套用那条判据的结果是：用户敲了命令、看到「[预演]」，
    // 以为结清了，实际什么都没发生。
    if a.has("dry-run") {
        println!("[预演] 将结清 {name}");
        println!("结论: {decision}");
        println!("去向: {}", dst.display());
        println!("追加: {} 字节", seg.len());
        return Ok(0);
    }

    ensure_dir(&closed)?;
    // 先写新位置、复读确认，最后才删原件：中途断掉的结果是「原件还在、副本也在」，
    // 而不是「原件没了、新件没写成」。
    write_atomic(&dst, &next)?;
    let back = read_str(&dst)?;
    if back != next {
        return Err(EE::Integrity(format!(
            "结清写入复读不一致: {} 期望 {} 字节，实读 {} 字节",
            dst.display(),
            next.len(),
            back.len()
        )));
    }
    std::fs::remove_file(&src)
        .map_err(|e| EE::Io(format!("原件删除失败 {}: {e}", src.display())))?;

    let body = format!(
        "结清待决暂存件 staging/{name}\n\n结论: {decision}\n{}去向: staging/closed/{name}\n\
原文保留未删，只在末尾追加了结清段。",
        ref_id.as_ref().map(|r| format!("依据记录: {r}\n")).unwrap_or_default()
    );
    let summary = format!("结清待决暂存件 {name}");
    let target = format!("staging/closed/{name}");
    crate::cmd_admin::write_commit(
        &root,
        &crate::cmd_admin::CommitInfo {
            slug: "staging-resolve",
            operation: "REPLACE",
            library: "NONE",
            summary: &summary,
            target: &target,
            scope: "移动 1 件待决暂存件并在末尾追加结论，不改动任何记录",
            body: &body,
        },
    )?;

    if a.has("json") {
        println!(
            "{{\"ok\":true,\"name\":{},\"moved_to\":{},\"decision\":{}}}",
            jstr(&name),
            jstr(&dst.display().to_string()),
            jstr(&decision)
        );
    } else {
        println!("已结清: {name}");
        println!("结论: {decision}");
        println!("去向: {}", dst.display());
    }
    Ok(0)
}

/// `review` — promotion proposals and their outcomes.
pub fn cmd_review(a: &Args) -> R<i32> {
    note_dir(a, "review", "晋升记录", "review")
}

/// `history` — commits touching a record, newest first, paged.
pub fn cmd_history(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let needle = a.pos.first().cloned().unwrap_or_default();
    let dir = root.join("commits");
    // Sort by the commit's own `TIME=` field, not by filename.
    //
    // The live library mixes two filename schemes: the old `0001-...` sequence
    // and the current `<stamp>-...`. Sorting names put the sequence after the
    // timestamps, and within one timestamp put `-1` before the bare name --
    // 44 of 76 rows landed in the wrong position. The file already records
    // when it happened, so ask the file.
    let mut rows: Vec<(i64, String, String, Option<usize>, Option<u64>)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            // 读不出来 = 量不出来，不是 0 字节 0 行。
            let raw = read_str(&p).ok();
            let txt = raw.clone().unwrap_or_default();
            if !needle.is_empty() && !txt.contains(&needle) && !name.contains(&needle) {
                continue;
            }
            let stamp = commit_time(&p, &txt);
            let first = txt
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim_start_matches('#')
                .trim()
                .to_string();
            rows.push((
                stamp,
                name,
                first,
                raw.as_ref().and_then(|t| count_lines_from(t.as_bytes())),
                raw.as_ref().map(|t| t.len() as u64),
            ));
        }
    }
    // Newest first; the name is the tie-break so equal timestamps stay stable.
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let pg = Page::from_args(a, rows.len());
    if a.has("json") {
        let items = rows
            .iter()
            .skip(pg.start())
            .take(pg.per.min(rows.len()))
            .map(|(ts, n, t, ln, by)| {
                format!(
                    "{{\"name\":{},\"title\":{},{}}}",
                    jstr(n),
                    jstr(t),
                    meta_json(*ts, *by, *ln)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!("{{{},\"commits\":[{items}]}}", pg.json_fields());
        return Ok(0);
    }
    for (ts, n, t, ln, by) in rows.iter().skip(pg.start()).take(pg.per.min(rows.len())) {
        println!("{}  {n:<48}  {t}", crate::util::row_meta(*ts, *by, *ln));
    }
    println!("{}", pg.bar());
    Ok(0)
}

/// `diff <a> <b>` — field-level difference between two records.
pub fn cmd_diff(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    if a.pos.len() < 2 {
        return usage("diff 需要两条记录");
    }
    let (p1, l1) = resolve_record(&root, &a.pos[0])?;
    let (p2, l2) = resolve_record(&root, &a.pos[1])?;
    let r1 = Record::parse(&p1, l1)?;
    let r2 = Record::parse(&p2, l2)?;
    let mut keys: Vec<String> = r1.fields.iter().map(|(k, _)| k.clone()).collect();
    for (k, _) in &r2.fields {
        if !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    let mut changes: Vec<(String, String, String)> = Vec::new();
    for k in &keys {
        let v1 = r1.get(k);
        let v2 = r2.get(k);
        if v1 != v2 {
            changes.push((k.clone(), v1, v2));
        }
    }
    let body_changed = r1.body != r2.body;
    let n = changes.len() + usize::from(body_changed);
    if a.has("json") {
        let items = changes
            .iter()
            .map(|(k, a1, b1)| {
                format!(
                    "{{\"field\":{},\"left\":{},\"right\":{}}}",
                    jstr(k),
                    jstr(a1),
                    jstr(b1)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"left\":{},\"right\":{},\"differences\":{n},\"fields\":[{items}],\"body_changed\":{},\"body_bytes\":[{},{}]}}",
            jstr(&r1.id()),
            jstr(&r2.id()),
            body_changed,
            r1.body.len(),
            r2.body.len()
        );
        return Ok(0);
    }
    for (k, v1, v2) in &changes {
        println!("~ {k}");
        if !v1.is_empty() {
            println!("  - {v1}");
        }
        if !v2.is_empty() {
            println!("  + {v2}");
        }
    }
    if body_changed {
        println!("~ 正文  {} 字节 → {} 字节", r1.body.len(), r2.body.len());
    }
    if n == 0 {
        println!("无差异");
    } else {
        println!("共 {n} 处差异");
    }
    Ok(0)
}

/// `export` — dump records in a machine-readable form, paged.
pub fn cmd_export(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let lib = crate::util::lib_from(a);
    let fmt = a.get("format").unwrap_or("jsonl").to_string();
    // `--format` is a free-form string, so an unrecognised value used to fall
    // through to the markdown branch. Asking for `json` therefore returned raw
    // `[CANONICAL_MEMORY]` blocks: valid text, wrong type, and nothing said so.
    // A format the tool cannot produce is a usage error, not a silent downgrade.
    if !matches!(fmt.as_str(), "jsonl" | "json" | "md" | "markdown") {
        return crate::util::usage(format!(
            "未知导出格式 {fmt}（可选 jsonl / json / md）"
        ));
    }
    let mut out: Vec<String> = Vec::new();
    let libs: Vec<Lib> = match lib {
        Some(l) => vec![l],
        None => vec![Lib::Canonical, Lib::Candidate],
    };
    for l in libs {
        for rec in load_lib(&root, l) {
            if rec.status() == "DELETED" && !a.has("include-deleted") {
                continue;
            }
            if fmt == "jsonl" || fmt == "json" {
                let fields = rec
                    .fields
                    .iter()
                    .map(|(k, v)| format!("{}:{}", jstr(k), jstr(v)))
                    .collect::<Vec<_>>()
                    .join(",");
                out.push(format!(
                    "{{\"path\":{},\"lib\":{},\"fields\":{{{fields}}},\"body\":{}}}",
                    jstr(&rel(&root, &rec.path)),
                    jstr(l.name()),
                    jstr(&rec.body)
                ));
            } else {
                out.push(rec.render());
            }
        }
    }
    // `json` is one array, `jsonl` is one document per line. Both carry the same
    // objects; the difference is whether a streaming reader can take a line at
    // a time without buffering the whole export.
    // Joined on a bare comma so the array occupies one line. Multi-line JSON is
    // legal, but the whole point of `--json` here is that a consuming script can
    // treat every line it reads as one complete value; a pretty-printed array
    // breaks that for no benefit.
    let text = if fmt == "json" {
        format!("[{}]", out.join(","))
    } else {
        out.join(if fmt == "jsonl" { "\n" } else { "\n\n" })
    };
    if let Some(dest) = a.get("out") {
        let dp = expand_tilde(dest);
        write_atomic(&dp, &text)?;
        println!("{} 条 → {}", out.len(), dp.display());
        return Ok(0);
    }
    let pg = Page::from_args(a, out.len());
    let page: Vec<&String> = out.iter().skip(pg.start()).take(pg.per.min(out.len())).collect();
    if fmt == "json" {
        println!("[{}]", page.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(","));
    } else {
        for line in page {
            println!("{line}");
        }
    }
    // The paging footer goes to stderr under `--json`.
    //
    // It is prose, and it was landing as the last line of a JSONL stream. A
    // reader that parses line by line got 30 good records and then a
    // "第 1/17 页" line that failed to parse -- with no way to tell a real
    // corruption from the tool's own footer. On stderr it stays available to a
    // human and out of the data.
    if pg.sliced() {
        if a.has("json") {
            eprintln!("{}", pg.bar());
        } else {
            println!("{}", pg.bar());
        }
        // Say where the rest is. A caller that only reads stdout would
        // otherwise silently receive page 1 and believe it was the whole
        // library; the count it needs is also on stderr as a document.
        if a.has("json") {
            eprintln!(
                "{{\"page\":{},\"per_page\":{},\"total\":{},\"pages\":{}}}",
                pg.page, pg.per, pg.total, pg.pages
            );
        }
    }
    Ok(0)
}

/// `tree` — branch -> record counts, paged.
pub fn cmd_tree(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    // Load each library's records once and index them by basename, so a branch
    // can report when it was last active without re-reading its members.
    //
    // The alternative (stat every PATH entry per branch) turns one `tree` into
    // hundreds of syscalls, and the library is loaded here anyway: `tree` is
    // the "what is in here" command, and "how recently was each branch touched"
    // is half the answer to that question.
    // One index of every record, keyed both by full relative path and by bare
    // filename. Atlas PATH entries mix the two spellings.
    //
    // The old version stored only a timestamp and had to be extended twice
    // (bytes, then lines); a struct keeps the next question from needing
    // another rewrite of the same block.
    #[derive(Clone, Copy, Default)]
    struct Meta {
        updated: i64,
        bytes: usize,
        lines: usize,
        attach_count: usize,
        attach_bytes: u64,
    }
    let mut index: std::collections::HashMap<String, Meta> = std::collections::HashMap::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        for r in load_lib(&root, lib) {
            let a = crate::attach::scan(&r.path);
            let m = Meta {
                updated: r.updated(),
                bytes: r.raw.len(),
                lines: count_lines(&r.raw),
                attach_count: a.count(),
                attach_bytes: a.bytes,
            };
            index.insert(format!("{}/{}", lib.dir(), r.file_name()), m);
            index.insert(r.file_name(), m);
        }
    }
    let mut js: Vec<String> = Vec::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        let ap = root.join(lib.atlas());
        if !ap.exists() {
            continue;
        }
        let at = crate::atlas::Atlas::load(&ap)?;
        // 一行一个分支。用结构体而不是八元组：列序正跟着 `row_meta` 统一，
        // 下标一多，改一次顺序就要重新数"第几个才是体积"。
        struct Row {
            name: String,
            count: usize,
            status: String,
            when: i64,
            /// `None` = 这条分支的成员一条都没找到（PATH 指向已改名的文件）。
            /// 那不是"0 字节 0 行"，是"量不出来"——两者的区别正是这一栏存在的
            /// 意义：0 会让读者跳过一条其实很重的分支。
            bytes: Option<u64>,
            lines: Option<usize>,
            attach_count: usize,
            attach_bytes: u64,
        }
        let mut rows: Vec<Row> = at
            .branches
            .iter()
            .map(|b| {
                // Deduplicate by basename before summing: one record may be
                // listed twice under different spellings, and counting it twice
                // would inflate the branch. The same set backs `count()`, so the
                // member count and the totals always agree.
                let mut seen: Vec<String> = Vec::new();
                let mut latest = 0i64;
                let mut bytes = 0u64;
                let mut lines = 0usize;
                let mut measured = 0usize;
                let (mut attach_count, mut attach_bytes) = (0usize, 0u64);
                for p in b.paths() {
                    let bare = p.rsplit('/').next().unwrap_or(&p).to_string();
                    if seen.contains(&bare) {
                        continue;
                    }
                    seen.push(bare.clone());
                    let m = match index.get(&p).or_else(|| index.get(&bare)) {
                        Some(m) => *m,
                        // A PATH entry whose file is missing is real in this
                        // library (records get renamed). It is still claimed as a
                        // member -- it just contributes nothing measurable.
                        None => continue,
                    };
                    measured += 1;
                    if m.updated > latest {
                        latest = m.updated;
                    }
                    bytes += m.bytes as u64;
                    lines += m.lines;
                    attach_count += m.attach_count;
                    attach_bytes += m.attach_bytes;
                }
                let unmeasured = measured == 0 && !seen.is_empty();
                Row {
                    name: b.name(),
                    count: seen.len(),
                    status: b.get("STATUS"),
                    when: latest,
                    bytes: if unmeasured { None } else { Some(bytes) },
                    lines: if unmeasured { None } else { Some(lines) },
                    attach_count,
                    attach_bytes,
                }
            })
            .collect();
        rows.sort_by(|x, y| y.count.cmp(&x.count));
        let _ = &rows;

        // Hierarchy, computed from PARENT rather than from LEVEL.
        //
        // A card's declared LEVEL can disagree with its PARENT chain -- one of
        // them is then simply wrong, and the chain is what a reader can verify
        // by following lines upward. Walking PARENT also finds orphans (a
        // parent name that no card carries); those are reported rather than
        // silently promoted to top level, because a branch that vanishes from
        // the tree is exactly the disappearance this tool exists to catch.
        let parents: std::collections::HashMap<String, String> = at
            .branches
            .iter()
            .map(|b| (b.name(), b.parent()))
            .collect();
        let names: Vec<String> = at.branches.iter().map(|b| b.name()).collect();
        let orphans: Vec<String> = parents
            .iter()
            .filter(|(_, par)| !par.is_empty() && !names.contains(par))
            .map(|(n, par)| format!("{n} -> {par}"))
            .collect();
        let flat = a.has("flat") || a.has("no-tree");
        let pg = Page::from_args(a, rows.len());
        if a.has("json") {
            let items = rows
                .iter()
                .skip(pg.start())
                .take(pg.per.min(rows.len()))
                .map(|r| {
                    format!(
                        "{{\"branch\":{},\"count\":{},\"status\":{},{},\"attach_count\":{},\"attach_bytes\":{}}}",
                        jstr(&r.name),
                        r.count,
                        jstr(&r.status),
                        meta_json(r.when, r.bytes, r.lines),
                        r.attach_count,
                        r.attach_bytes
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            js.push(format!(
                "{{{},\"lib\":{},\"branches\":[{items}]}}",
                pg.json_fields(),
                jstr(lib.name())
            ));
            continue;
        }
        println!("== {}", lib.name());
        if flat || parents.values().all(|p| p.is_empty()) {
            // No hierarchy declared: the flat list. 列序跟 `row_meta` 一致：
            // 时间 / 体积 / 行数 在最前，条目数这类命令特有列接在后面。
            for r in rows.iter().skip(pg.start()).take(pg.per.min(rows.len())) {
                println!(
                    "{}  {:<28} {:>4}条  {:<8} {}",
                    crate::util::row_meta(r.when, r.bytes, r.lines),
                    r.name,
                    r.count,
                    r.status,
                    if r.attach_count == 0 {
                        String::new()
                    } else {
                        format!("{}件{}", r.attach_count, human_bytes(r.attach_bytes))
                    }
                );
            }
        } else {
            // Depth-first walk. Children are ordered by size so the biggest
            // subtree is read first -- the point of a tree view is deciding
            // where to look next, and that decision is made on volume.
            //
            // 元信息前缀始终从第 0 列起，树的缩进放在它右边：这样 `tree` 与
            // `list`/`branch`/`index` 并排看时前三列对得齐，同时子树仍然看得出
            // 层级。
            fn emit(
                name: &str,
                depth: usize,
                parents: &std::collections::HashMap<String, String>,
                rows: &[Row],
                out: &mut Vec<String>,
            ) {
                if let Some(r) = rows.iter().find(|r| r.name == name) {
                    let pad = "  ".repeat(depth);
                    let mark = if depth == 0 { "▸" } else { "·" };
                    out.push(format!(
                        "{}  {pad}{mark} {:<28} {:>4}条  {:<8} {}",
                        crate::util::row_meta(r.when, r.bytes, r.lines),
                        r.name,
                        r.count,
                        r.status,
                        if r.attach_count == 0 {
                            String::new()
                        } else {
                            format!("{}件{}", r.attach_count, human_bytes(r.attach_bytes))
                        }
                    ));
                }
                let mut kids: Vec<&String> = parents
                    .iter()
                    .filter(|(_, par)| *par == name)
                    .map(|(k, _)| k)
                    .collect();
                kids.sort_by(|x, y| {
                    let bx = rows.iter().find(|r| &r.name == *x).and_then(|r| r.bytes).unwrap_or(0);
                    let by_ = rows.iter().find(|r| &r.name == *y).and_then(|r| r.bytes).unwrap_or(0);
                    by_.cmp(&bx)
                });
                for k in kids {
                    emit(k, depth + 1, parents, rows, out);
                }
            }
            let mut out: Vec<String> = Vec::new();
            let mut roots: Vec<&String> = names
                .iter()
                .filter(|n| parents.get(*n).map(|p| p.is_empty()).unwrap_or(true))
                .collect();
            roots.sort_by(|x, y| {
                let bx = rows.iter().find(|r| &r.name == *x).and_then(|r| r.bytes).unwrap_or(0);
                let by_ = rows.iter().find(|r| &r.name == *y).and_then(|r| r.bytes).unwrap_or(0);
                by_.cmp(&bx)
            });
            for r in roots {
                emit(r, 0, &parents, &rows, &mut out);
            }
            // Paging applies to the rendered lines, not to branches: a subtree
            // is several lines, and slicing branches would cut a parent off its
            // children.
            let pg2 = Page::from_args(a, out.len());
            for line in out.iter().skip(pg2.start()).take(pg2.per.min(out.len())) {
                println!("{line}");
            }
            if !orphans.is_empty() {
                println!("  [悬挂 PARENT {} 处] {}", orphans.len(), orphans.join("、"));
            }
            println!("  {}", pg2.bar());
            continue;
        }
        println!("  {}", pg.bar());
    }
    if a.has("json") {
        println!("{{\"libs\":[{}]}}", js.join(","));
    }
    Ok(0)
}

/// `search` — high-performance content search with paging.
pub fn cmd_search(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let mut patterns: Vec<String> = Vec::new();
    let mut i = 0;
    while let Some(p) = a.pos.get(i) {
        patterns.push(p.clone());
        i += 1;
    }
    if let Some(extra) = a.get("and") {
        for part in extra.split(['|', '，', ',']) {
            let t = part.trim();
            if !t.is_empty() {
                patterns.push(t.to_string());
            }
        }
    }
    if patterns.is_empty() {
        return usage("用法: abook search <词> [更多词]  [--regex|--fuzzy|--literal]");
    }
    if a.has("semantic") || a.has("sem") || a.has("ask") {
        return semantic_search(a, &root, patterns);
    }
    // Unified cost tiers. The model picks how much it is willing to pay:
    //   L1 literal   byte prefilter + parallel scan, no decode of misses
    //   L2 fuzzy     bounded edit distance, CPU-heavy per candidate
    //   L3 semantic  in-memory BM25, no expansion
    //   L4 +expand   BM25 plus association expansion (default semantic)
    //   L5 deep      two expansion rounds over a wider seed set
    let level = a.num("level").unwrap_or(0) as u8;
    // `--deep` on its own has to reach the semantic path, not just widen it once
    // it is already there. It used to be read only inside `semantic_search`, so
    // `search 词 --deep` stayed on the literal tier and returned exactly what
    // `search 词` returned -- the most expensive tier silently cost the least and
    // found the least. The flag is the tier here, so it decides the route.
    if a.has("deep") && !patterns.is_empty() {
        return semantic_search(a, &root, patterns);
    }
    if level >= 3 && !patterns.is_empty() {
        return semantic_search(a, &root, patterns);
    }
    let mode = if a.has("regex") || a.has("re") {
        crate::search::Mode::Regex
    } else if a.has("fuzzy") || a.has("approx") || level == 2 {
        crate::search::Mode::Fuzzy
    } else if a.has("literal") || a.has("exact") || level == 1 {
        crate::search::Mode::Literal
    } else {
        crate::search::Mode::All
    };
    let opts = crate::search::Opts {
        patterns,
        mode,
        ci: !a.has("case"),
        fuzzy_k: a.num("distance").unwrap_or(1) as usize,
        fields: a
            .get("field")
            .map(|f| f.split(',').map(|x| x.trim().to_string()).collect())
            .unwrap_or_default(),
        body_only: a.get("field").map(|f| f == "body").unwrap_or(false),
        context: a.num("context").unwrap_or(0) as usize,
        max_hits: a.num("max").unwrap_or(0) as usize,
        min_score: a.num("min-score").unwrap_or(0) as i32,
        raw_lines: a.has("raw-lines") || a.has("lines") || a.has("whole-line"),
        lib: crate::util::lib_from(a),
        include_deleted: a.has("include-deleted"),
        include_promoted: !a.has("hide-promoted"),
    };
    let t0 = std::time::Instant::now();
    let hits = match crate::search::search(&root, &opts) {
        Ok(h) => h,
        Err(e) => return usage(e),
    };
    let ms = t0.elapsed().as_millis();
    let pg = Page::from_args(a, hits.len());
    if a.has("json") {
        let rows = hits
            .iter()
            .skip(pg.start())
            .take(pg.per.min(hits.len()))
            .map(|h| {
                format!(
                    "{{\"path\":{},\"score\":{},\"field\":{},\"line\":{},\"title\":{},\"snippet\":{},\"attach_count\":{},\"attach_bytes\":{},{}}}",
                    jstr(&h.path),
                    h.score,
                    jstr(&h.where_),
                    h.line,
                    jstr(&h.title),
                    jstr(&h.snippet),
                    h.attach_count,
                    h.attach_bytes,
                    meta_json(h.updated, Some(h.bytes as u64), Some(h.lines))
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"total\":{},\"ms\":{},\"results\":[{rows}]}}",
            hits.len(),
            ms
        );
        return Ok(if hits.is_empty() { 3 } else { 0 });
    }
    for h in hits.iter().skip(pg.start()).take(pg.per.min(hits.len())) {
        // Ranked by score, as asked; the date rides along for placement in time.
        let att = if h.attach_count == 0 {
            String::new()
        } else {
            format!("{}件{}", h.attach_count, human_bytes(h.attach_bytes))
        };
        // 行数这一列曾在这里是 `{:>7}`、在 `list` 是 `{:>4}`、在 `row_meta` 是
        // `{:>5}`：同一个含义三种宽度。现在三段元信息统一由 `row_meta` 铺开，
        // 命中分（这行真正的排序依据）接在后面，不与那三列抢位置。
        // 附件列与 `list` 同一处理：没有附件就不占位，空列与"字段缺失"在视觉上
        // 分不出来。
        let meta = crate::util::row_meta(h.updated, Some(h.bytes as u64), Some(h.lines));
        match if att.is_empty() { None } else { Some(att.as_str()) } {
            Some(a) => println!(
                "{meta}  {:>5}  {a:>12}  {}  [{}] {}",
                h.score,
                h.path,
                h.where_,
                truncate_chars(&h.title, 60)
            ),
            None => println!(
                "{meta}  {:>5}  {}  [{}] {}",
                h.score,
                h.path,
                h.where_,
                truncate_chars(&h.title, 60)
            ),
        }
        if !h.snippet.is_empty() {
            println!("       行{}  {}", h.line, h.snippet);
        }
    }
    println!("{}  {}ms", pg.bar(), ms);
    if hits.is_empty() {
        return Ok(3);
    }
    Ok(0)
}

/// `search --semantic` — BM25 + association expansion, rebuilt in memory.
fn semantic_search(a: &Args, root: &Path, patterns: Vec<String>) -> R<i32> {
    let q = patterns.join(" ");
    let libs: Vec<Lib> = crate::util::libs_from(a);
    let t0 = std::time::Instant::now();
    let idx = crate::semantic::Index::build(root, &libs);
    let build_ms = t0.elapsed().as_millis();
    let t1 = std::time::Instant::now();
    // Depth follows the requested tier unless overridden explicitly.
    let level = a.num("level").unwrap_or(0) as u8;
    let depth: u8 = if a.has("deep") || level >= 5 {
        2
    } else if a.has("no-expand") || level == 3 {
        0
    } else {
        1
    };
    let rows = idx.query_depth(&q, depth, a.num("max").unwrap_or(20) as usize);
    let q_ms = t1.elapsed().as_millis();
    let pg = Page::from_args(a, rows.len());
    if a.has("json") {
        let body = rows
            .iter()
            .skip(pg.start())
            .take(pg.per.min(rows.len()))
            .map(|(i, sc, why)| {
                let d = &idx.docs[*i];
                format!(
                    "{{\"path\":{},\"score\":{:.4},\"title\":{},\"via\":{}}}",
                    jstr(&d.path),
                    sc,
                    jstr(&d.title),
                    jstr(why)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"query\":{},\"total\":{},\"terms\":{},\"build_ms\":{},\"query_ms\":{},\"results\":[{body}]}}",
            jstr(&q),
            rows.len(),
            idx.total_terms,
            build_ms,
            q_ms
        );
        return Ok(if rows.is_empty() { 3 } else { 0 });
    }
    for (i, sc, why) in rows.iter().skip(pg.start()).take(pg.per.min(rows.len())) {
        let d = &idx.docs[*i];
        println!(
            "{:>7.3}  {}  [{}] {}",
            sc,
            d.path,
            d.lib.name(),
            truncate_chars(&d.title, 64)
        );
        if !why.is_empty() {
            println!("         {why}");
        }
    }
    println!(
        "{}  索引 {} 项 / 建 {}ms / 查 {}ms{}",
        pg.bar(),
        idx.total_terms,
        build_ms,
        q_ms,
        match depth {
            0 => "（纯词频）",
            1 => "（含关联扩展）",
            _ => "（深度扩展）",
        }
    );
    if rows.is_empty() {
        return Ok(3);
    }
    Ok(0)
}

/// `grep` — line-oriented search over raw record text.
pub fn cmd_grep(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let pat = a.need_pos(0, "搜索文本")?;
    let ctx = a.num("context").unwrap_or(0) as usize;
    // Structured hits first, so the text and JSON renderings cannot disagree
    // about what was found or how many.
    //
    // Pattern folded once per scan, not once per line.
    //
    // The haystack fold stays `str::to_lowercase`, deliberately. Reusing one
    // buffer for it means reimplementing that function, and on this toolchain it
    // is *not* `chars().flat_map(char::to_lowercase)`: std maps `Σ` by context
    // (`ς` for Final_Sigma, `σ` otherwise), so a char-wise fold silently stopped
    // matching lines ending in a capital sigma. Measured: `grep ς` lost the line
    // `ΣΟΦΟΣ 同样词尾。`. Dropping real hits to save an allocation is not a trade
    // worth making. An ASCII-only fast path was also tried and measured slower
    // (std's ASCII conversion is vectorized; a hand-written loop is not).
    //
    // This is not where `grep` spends its time anyway: `list --q <no match>`
    // loads the same libraries and prints nothing, and it already costs the same
    // ~108 ms that `grep` does.
    let pat_lc = pat.to_lowercase();
    let mut found: Vec<(String, usize, String)> = Vec::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        for rec in load_lib(&root, lib) {
            for (i, line) in rec.raw.lines().enumerate() {
                if line.to_lowercase().contains(&pat_lc) {
                    found.push((
                        rel(&root, &rec.path),
                        i + 1,
                        truncate_chars(line, 200),
                    ));
                }
            }
        }
    }
    let pg = Page::from_args(a, found.len());
    if a.has("json") {
        let items = found
            .iter()
            .skip(pg.start())
            .take(pg.per.min(found.len()))
            .map(|(p, ln, t)| {
                format!(
                    "{{\"path\":{},\"line\":{ln},\"text\":{}}}",
                    jstr(p),
                    jstr(t)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{{},\"pattern\":{},\"total\":{},\"hits\":[{items}]}}",
            pg.json_page_only(),
            jstr(&pat),
            found.len()
        );
        return Ok(if found.is_empty() { 3 } else { 0 });
    }
    let mut printed = 0usize;
    for (idx, (p, ln, t)) in found.iter().enumerate() {
        if idx < pg.start() || idx >= pg.start() + pg.per {
            continue;
        }
        println!("{p}:{ln}: {t}");
        printed += 1;
        if ctx > 0 {
            if let Ok(txt) = std::fs::read_to_string(root.join(p)) {
                let lines: Vec<&str> = txt.lines().collect();
                let lo = (ln - 1).saturating_sub(ctx);
                let hi = ((ln - 1) + ctx + 1).min(lines.len());
                for j in lo..hi {
                    if j != ln - 1 {
                        println!("    {}  {}", j + 1, truncate_chars(lines[j], 140));
                    }
                }
            }
        }
    }
    let _ = printed;
    println!("{}", pg.bar());
    if found.is_empty() {
        return Ok(3);
    }
    Ok(0)
}

/// `list` — filtered, paged record listing.
pub fn cmd_list(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let f = ListFilter::from_args(a);
    let mut rows: Vec<Record> = Vec::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        for rec in load_lib(&root, lib) {
            if f.accept(&rec, a) {
                rows.push(rec);
            }
        }
    }
    // Newest *touched* first, not newest created and not newest filename.
    //
    // Sorting by filename put a record edited five minutes ago below every
    // record created after it, so the list answered "what exists" but not
    // "what have I been working on" -- the question a reader actually has when
    // they open a listing. Editing a record is the strongest available signal
    // that it was the most recent thing on someone's mind.
    //
    // The name stays as the tie-break so equal timestamps keep a stable order;
    // without it, two records written in the same second could swap places
    // between runs and a paged read would skip or repeat rows.
    rows.sort_by(|x, y| {
        y.updated()
            .cmp(&x.updated())
            .then_with(|| y.file_name().cmp(&x.file_name()))
    });
    // Paging first, attachments second.
    //
    // The attachment column is only rendered for the rows on the current page,
    // so only those rows are scanned. Indexing every row up front meant
    // `list --per-page 1` probed 962 records' worth of `-refs` candidates --
    // 4810 statx calls to fill a column that then showed one row (measured:
    // 6758 statx for a one-row listing).
    //
    // The index stays keyed by file name, and it still covers exactly the rows
    // it renders: the two libraries hold disjoint record names (checked over the
    // whole library: 0 shared basenames), so a page-scoped index cannot shadow a
    // row that is on the page. The counts themselves come from one scan per row
    // either way, so the column is still one consistent view.
    let pg = Page::from_args(a, rows.len());
    let page: Vec<&Record> = rows
        .iter()
        .skip(pg.start())
        .take(pg.per.min(rows.len()))
        .collect();
    let mut att: std::collections::HashMap<String, crate::attach::Files> =
        std::collections::HashMap::new();
    for r in &page {
        let f = crate::attach::scan(&r.path);
        if !f.is_empty() {
            att.insert(r.file_name(), f);
        }
    }
    let att_of = |rec: &Record| -> crate::attach::Files {
        att.get(&rec.file_name()).cloned().unwrap_or_default()
    };
    if a.has("json") {
        // A caller that asked for JSON and got a table would silently parse
        // nothing, so every command that advertises --json has to branch here.
        let items = page
            .iter()
            .copied()
            .map(|rec| {
                format!(
                    "{{\"path\":{},\"lib\":{},\"id\":{},\"title\":{},\"status\":{},\"kind\":{},\"maturity\":{},\"created\":{},{}}}",
                    jstr(&rel(&root, &rec.path)),
                    jstr(rec.lib.name()),
                    jstr(&rec.id()),
                    jstr(&rec.title()),
                    jstr(&rec.status()),
                    jstr(&rec.kind()),
                    jstr(&rec.get("MATURITY")),
                    rec.created(),
                    meta_json(
                        rec.updated(),
                        Some(rec.raw.len() as u64),
                        Some(count_lines(&rec.raw))
                    )
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!("{{{},\"records\":[{items}]}}", pg.json_fields());
        return Ok(0);
    }
    // 文本输出里用短号代替完整路径。
    //
    // 完整编号 `canonical/records/C-20261029T101635-f449f353.md` 是 48 字符，
    // 每行都印一遍，一页 30 行就是 1.4KB 的纯编号。短号 `C123` 是 4 字符，
    // 而且不暴露创建时间与内容哈希。要看完整编号用 `abook num --of C123`。
    let nums = crate::num::Numbers::load_or_build(&root);
    for rec in page.iter().copied() {
        // 元信息一律走 `row_meta`：此处自己写 `{:>4}` 放过行数，于是从"行数"往后
        // 整行相对其它命令左移一格，并排看就错位。附件列是命令特有信息，接在
        // 统一前缀之后。
        // 附件列只在真有附件时才占位。
        //
        // 原先无条件打印 `{:>12}`：整个库里绝大多数记录没有附件，于是每一行都
        // 是一段 12 字符的空白，看起来像排版坏了或者有字段没填。宽表格里"空列"
        // 与"字段缺失"在视觉上无法区分，读者只会去怀疑工具。
        let att = {
            let f = att_of(rec);
            if f.is_empty() {
                None
            } else {
                Some(format!("{}件{}", f.count(), human_bytes(f.bytes)))
            }
        };
        let meta = crate::util::row_meta(
            rec.updated(),
            Some(rec.raw.len() as u64),
            Some(count_lines(&rec.raw)),
        );
        match att {
            Some(att) => println!(
                "{meta}  {att:>12}  {}  [{}] {}",
                short_path(&nums, &root, &rec.path),
                rec.lib.name(),
                truncate_chars(&rec.title(), 70)
            ),
            None => println!(
                "{meta}  {}  [{}] {}",
                short_path(&nums, &root, &rec.path),
                rec.lib.name(),
                truncate_chars(&rec.title(), 70)
            ),
        }
    }
    // 零命中时说清楚"为什么是零"。
    //
    // `list --status CANDIDATE` 返回「共 0 条」曾让人以为候选库是空的。真相
    // 是：一个记录属于哪个库由**目录**决定，候选记录的头部不会写
    // `STATUS=CANDIDATE`。筛选零命中，可能确实没有匹配，也可能是这个条件本身
    // 不适用于这套数据模型——两者用同一句话回答，读的人就会去修一个不存在的
    // 毛病。没有输出比报错更容易误导，因为脚本和人都把"空"当成结论。
    if rows.is_empty() {
        if let Some(st) = a.get("status") {
            let up = st.to_uppercase();
            if up == "CANDIDATE" || up == "CANONICAL" {
                crate::say_line!(
                    "提示：`{up}` 不是 STATUS 取值——记录属于哪个库由所在目录决定。\n      \
                     用 `abook list --lib {}` 列出该库，别用 --status。",
                    if up == "CANDIDATE" { "candidate" } else { "canonical" }
                );
            } else {
                crate::say_line!("提示：没有 STATUS={up} 的记录；可用取值见 `abook stats`。");
            }
        } else if a.get("q").is_some() || a.get("tag").is_some() || a.get("kind").is_some()
            || a.get("lib").is_some()
        {
            crate::say_line!("提示：这个筛选条件没有匹配到任何记录。");
        }
    }
    println!("{}", pg.bar());
    Ok(0)
}

/// `budget` — estimate what a recall would cost, mirroring the prompt's read
/// budget rule so the model can check before it reads.
pub fn cmd_budget(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let preload = [
        "Global.md",
        "Index.md",
        "canonical/Atlas.md",
        "candidate/Atlas.md",
    ];
    let mut total = 0usize;
    // Measure once, then render either form: recomputing sizes per branch is
    // how the two outputs drift apart.
    let mut parts: Vec<(String, usize)> = Vec::new();
    for f in preload {
        let p = root.join(f);
        let n = std::fs::metadata(&p).map(|m| m.len() as usize).unwrap_or(0);
        total += n;
        parts.push((f.to_string(), n));
    }
    let one = match a.pos.first() {
        Some(sel) => {
            let (path, lib) = resolve_record(&root, sel)?;
            let rec = Record::parse(&path, lib)?;
            Some((rel(&root, &path), rec.raw.len()))
        }
        None => None,
    };
    if a.has("json") {
        let files = parts
            .iter()
            .map(|(k, v)| format!("{}:{}", jstr(k), v))
            .collect::<Vec<_>>()
            .join(",");
        let rec = match &one {
            Some((p, n)) => format!(
                "{{\"path\":{},\"bytes\":{},\"limit\":8192,\"ok\":{}}}",
                jstr(p),
                n,
                *n <= 8192
            ),
            None => "null".to_string(),
        };
        println!(
            "{{\"files\":{{{files}}},\"total\":{total},\"limit\":12288,\"ok\":{},\"record\":{rec}}}",
            total <= 12288
        );
        return Ok(0);
    }
    println!("预载预算");
    for (f, n) in &parts {
        println!("  {f:<24} {n:>7} B");
    }
    println!("  合计 {total} B（规则上限 12288 B: {}）", if total <= 12288 { "通过" } else { "超限" });
    if let Some((path, n)) = &one {
        println!("单条 {path}  {n} B（规则上限 8192 B: {}）",
            if *n <= 8192 { "通过" } else { "超限" });
    }
    Ok(0)
}

// ---------------------------------------------------------------- 单元测试
#[cfg(test)]
mod tests {
    use super::*;

    /// 未知维度不许伪装成一个数字。
    ///
    /// `index --json` 曾经把读不出来的记录报成 `0 字节 0 行、更新于 1970`，
    /// 而文本侧同一份数据说的是"元信息不可得"。这条测试把两侧钉在一起：
    /// 文本三个维度都只能是 `-`，JSON 三个维度都只能是 `null`。
    #[test]
    fn unknown_meta_is_dash_and_null_not_zero() {
        let text = crate::util::row_meta(0, None, None);
        assert!(
            !text.chars().any(|c| c.is_ascii_digit()),
            "未知维度在文本里被写成了数字: {text:?}"
        );
        assert_eq!(text.matches('-').count(), 3, "三个维度都该是 `-`: {text:?}");
        assert_eq!(
            meta_json(0, None, None),
            "\"bytes\":null,\"lines\":null,\"updated\":null,\"when\":\"-\""
        );
        // `:0` 会同时命中 `"updated":0` 和 `"bytes":0`；两者都是这条要禁的。
        assert!(!meta_json(0, None, None).contains(":0"));
    }

    /// 0 是测量值，不能被当成"未知"处理掉。
    #[test]
    fn zero_is_a_measurement_but_missing_is_not() {
        let text = crate::util::row_meta(0, Some(0), Some(0));
        assert!(text.contains("0B"), "{text:?}");
        assert!(text.contains("0行"), "{text:?}");
        let j = meta_json(0, Some(0), Some(0));
        // 只针对被测的两个维度断言。`when=0` 是"时间未知"，它输出
        // `"updated":null` 是对的——原来这里写的是 `!j.contains("null")`，
        // 把那个正确的 null 也算成"0 被写成未知"，于是测试自己在失败。
        // 断言的范围要正好圈住被测对象，宽一点就会把邻列的正确行为当错误。
        assert!(!j.contains("\"bytes\":null"), "0 字节被写成了未知: {j}");
        assert!(!j.contains("\"lines\":null"), "0 行被写成了未知: {j}");
        assert!(j.contains("\"bytes\":0"), "{j}");
        assert!(j.contains("\"lines\":0"), "{j}");
    }

    /// 行数只有一套算法：与 `str::lines()` 逐个对拍，并钉住"末尾没换行不丢一行"。
    #[test]
    fn line_counting_is_one_algorithm() {
        let cases = [
            "", "a", "a\n", "a\nb", "a\nb\n", "\n", "\n\n", "a\n\nb\n", "a\r\nb\r\n", "a\r\nb",
        ];
        for t in cases {
            assert_eq!(count_lines(t), t.lines().count(), "count_lines({t:?})");
            assert_eq!(count_lines_bytes(t.as_bytes()), t.lines().count(), "count_lines_bytes({t:?})");
        }
        // 手改的文件末尾没有换行：两种旧实现在这里差 1，现在必须一致。
        assert_eq!(count_lines("x\ny"), 2);
        assert_eq!(count_lines("x\ny\n"), 2);
    }

    /// 二进制没有行数；文件形态与字节形态必须给同一个答案。
    #[test]
    fn binary_has_no_line_count() {
        assert_eq!(count_lines_from(&b"\x00\x01\x02"[..]), None);
        // 没有 NUL 但也不是 UTF-8 的随机字节：不能数出行数来。
        assert_eq!(count_lines_from(&[0xFFu8, 0xFE, b'\n'][..]), None);
        assert_eq!(count_lines_from(&[0x89u8, b'P', b'N', b'G', b'\n'][..]), None);
        // 中文与 emoji 是文本，跨块边界也不能被误判成二进制。
        let mut long = String::new();
        for _ in 0..5000 {
            long.push_str("中文测试🙂\n");
        }
        assert_eq!(count_lines_from(long.as_bytes()), Some(5000));
        let d = std::env::temp_dir().join("abook-count-lines-test");
        let _ = std::fs::create_dir_all(&d);
        let txt = d.join("a.md");
        std::fs::write(&txt, "one\ntwo\nthree").expect("写临时文件");
        assert_eq!(count_lines_of_file(&txt), Some(3));
        let bin = d.join("b.bin");
        std::fs::write(&bin, [0u8, 1, 2, b'\n']).expect("写临时文件");
        assert_eq!(count_lines_of_file(&bin), None);
        assert_eq!(count_lines_of_file(&d.join("不存在")), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    // ------------------------------------------------------------ 结清待决件

    /// 每个用例一个独立目录：结清会移动文件，共用目录会让用例之间互相影响。
    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("abook-staging-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("staging")).expect("建临时目录");
        std::fs::create_dir_all(d.join("commits")).expect("建临时目录");
        d
    }

    fn args(root: &Path, tail: &[&str]) -> Args {
        let mut v = vec!["staging".to_string(), "--root".to_string(), root.display().to_string()];
        v.extend(tail.iter().map(|s| s.to_string()));
        Args::parse(&v).expect("参数解析")
    }

    /// 结清 = 原文一字不动 + 结尾追加结论 + 移进 closed/。
    ///
    /// 这条测试钉的是「原文不能丢」：结清的语义是把当时的认知存档，不是在得出
    /// 结论之后把问题本身抹掉。如果哪天有人把它实现成"写个结论然后把原件删了"，
    /// 三个月后就没人知道当初在纠结什么。
    #[test]
    fn resolve_keeps_original_bytes_and_appends_decision() {
        let root = scratch("keep");
        let orig = "# 待决\n\n- 待决项：要不要收\n";
        std::fs::write(root.join("staging/p.md"), orig).expect("写待决件");

        let rc = cmd_staging(&args(&root, &["resolve", "p", "--decision", "保持现状"])).expect("结清");
        assert_eq!(rc, 0);

        assert!(!root.join("staging/p.md").exists(), "原位置不该还留着");
        let got = std::fs::read_to_string(root.join("staging/closed/p.md")).expect("读结清件");
        assert!(got.starts_with(orig.trim_end()), "原文被改动了: {got:?}");
        assert!(got.contains("## 结清"), "没有结清段: {got:?}");
        assert!(got.contains("- 决定：保持现状"), "结论没落进去: {got:?}");
        // 结论段与原文之间要有空行，否则标题贴在上一个字下面，读起来像原文的一部分。
        assert!(got.contains("\n\n## 结清\n\n"), "结清段与原文之间没有空行: {got:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 结清可以被再问一次，答案来自记录而不是重新决定。
    ///
    /// 幂等不是"再写一次"，是"已经结过了，把当时的结论念出来"。否则第二次调用
    /// 会把 closed/ 里的文件再追加一段，两次结论叠在一起，读者不知道该信哪条。
    #[test]
    fn resolve_is_idempotent_and_reports_the_recorded_decision() {
        let root = scratch("idem");
        std::fs::write(root.join("staging/p.md"), "# 待决\n").expect("写待决件");
        cmd_staging(&args(&root, &["resolve", "p", "--decision", "保持现状"])).expect("首次结清");
        let once = std::fs::read_to_string(root.join("staging/closed/p.md")).expect("读");

        let rc = cmd_staging(&args(&root, &["resolve", "p", "--decision", "改成别的"])).expect("二次结清");
        assert_eq!(rc, 0, "已结清的件再结一次不该报错");
        let twice = std::fs::read_to_string(root.join("staging/closed/p.md")).expect("读");
        assert_eq!(once, twice, "第二次调用改写了已存档的文件");
        assert!(!twice.contains("改成别的"), "第二次的结论覆盖了当时的决定");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 没有结论就不许结清。
    ///
    /// 允许"结清但不给结论"等于给了一条把待决项藏进子目录的快捷方式：数字归零，
    /// 而问题还在。退码 2 是这条规矩的可观察形式。
    #[test]
    fn resolve_without_decision_is_refused() {
        let root = scratch("nodec");
        std::fs::write(root.join("staging/p.md"), "# 待决\n").expect("写待决件");
        let e = cmd_staging(&args(&root, &["resolve", "p"])).expect_err("无结论必须被拒");
        assert!(matches!(e, EE::Usage(_)), "该是用法错误，实际 {e:?}");
        assert!(root.join("staging/p.md").exists(), "被拒时不该移动文件");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 只处理 staging/ 下的一层文件名，不接受路径。
    ///
    /// 这条不是洁癖：命令里带着 `..` 就能把库外任意文件追加一段再删掉，而它的
    /// 用途只是移一件待决件。
    #[test]
    fn resolve_rejects_paths() {
        let root = scratch("path");
        for bad in ["../escape", "a/b", "..", "."] {
            let e = cmd_staging(&args(&root, &["resolve", bad, "--decision", "x"]))
                .expect_err("路径必须被拒");
            assert!(matches!(e, EE::Usage(_)), "{bad} 该被拒，实际 {e:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 两边同时存在 = 上一次只做了一半，停下来，不要猜。
    ///
    /// 这时覆盖 closed/ 会丢掉已追加的结论，删原件会丢掉原文；两个方向都会静默
    /// 损坏数据，所以必须是完整性错误（退码 5）而不是"我来帮你选一个"。
    #[test]
    fn resolve_refuses_when_both_copies_exist() {
        let root = scratch("both");
        std::fs::create_dir_all(root.join("staging/closed")).expect("建 closed");
        std::fs::write(root.join("staging/p.md"), "# 原件\n").expect("写");
        std::fs::write(root.join("staging/closed/p.md"), "# 已结清\n").expect("写");
        let e = cmd_staging(&args(&root, &["resolve", "p", "--decision", "x"]))
            .expect_err("两边都在必须被拒");
        assert!(matches!(e, EE::Integrity(_)), "该是完整性错误，实际 {e:?}");
        assert_eq!(
            std::fs::read_to_string(root.join("staging/p.md")).expect("读"),
            "# 原件\n",
            "被拒时不该动任何一份"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 找不到的件报退码 3，并把现有件列出来。
    #[test]
    fn resolve_missing_names_what_is_there() {
        let root = scratch("missing");
        std::fs::write(root.join("staging/other.md"), "# 另一件\n").expect("写");
        let e = cmd_staging(&args(&root, &["resolve", "nope", "--decision", "x"]))
            .expect_err("找不到必须报错");
        match e {
            EE::NotFound(m) => assert!(m.contains("other.md"), "没把现有件告诉调用方: {m}"),
            other => panic!("该是未找到，实际 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 预演一件都不改；真跑才改。
    ///
    /// 这里的判据不能用 `Args::dry_run()`——那条是"没有 --apply 就当预演"，套在
    /// 结清上会让一条命令式的动作静默变成空操作。所以 `--dry-run` 必须逐个验。
    #[test]
    fn dry_run_changes_nothing() {
        let root = scratch("dry");
        let orig = "# 待决\n";
        std::fs::write(root.join("staging/p.md"), orig).expect("写");
        let rc = cmd_staging(&args(&root, &["resolve", "p", "--decision", "x", "--dry-run"]))
            .expect("预演");
        assert_eq!(rc, 0);
        assert_eq!(
            std::fs::read_to_string(root.join("staging/p.md")).expect("读"),
            orig,
            "预演移动了原件"
        );
        assert!(!root.join("staging/closed/p.md").exists(), "预演写出了结清件");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 结论里的换行要被压平，否则只读到第一行的人会以为决策就是那半句。
    #[test]
    fn decision_newlines_are_flattened() {
        let root = scratch("nl");
        std::fs::write(root.join("staging/p.md"), "# 待决\n").expect("写");
        cmd_staging(&args(&root, &["resolve", "p", "--decision", "第一句\n第二句"]))
            .expect("结清");
        let got = std::fs::read_to_string(root.join("staging/closed/p.md")).expect("读");
        assert!(got.contains("- 决定：第一句 第二句"), "换行没被压平: {got:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 归档件必须仍能被 `--show` 读到，只敲文件名就行。
    ///
    /// 结清把文件移进 `closed/` 之后，只给文件名会读不出来——文件在库里，工具却说
    /// 没有。归档不是删除：待决数回答"还有几件在等人"，`closed/` 回答"当初是怎么
    /// 定的"。如果读不回来，后一半就没有落地的地方，结清就等于删除。
    #[test]
    fn archived_note_is_still_readable_by_bare_name() {
        let root = scratch("show-archived");
        std::fs::write(root.join("staging/p.md"), "# 待决\n- 项：要不要\n").expect("写");
        cmd_staging(&args(&root, &["resolve", "p", "--decision", "保持现状"])).expect("结清");

        // 只给文件名：应当落到 closed/ 里那一份。
        let rc = cmd_staging(&args(&root, &["--show", "p.md"])).expect("读归档件");
        assert_eq!(rc, 0, "只给文件名读不到归档件");
        // 显式带 closed/ 前缀也要能读，两条路径不能互相排斥。
        let rc = cmd_staging(&args(&root, &["--show", "closed/p.md"])).expect("读归档件");
        assert_eq!(rc, 0, "带 closed/ 前缀反而读不到");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 找不到的件是 NotFound（退码 3），不是用法错误。
    ///
    /// 改成退码 3 之前这里是 `usage`（退码 2）：打错名字和命令敲错被报成同一件
    /// 事，调用方只能靠读自然语言分辨。`--show` 是查询，查不到就是查不到。
    #[test]
    fn show_missing_note_is_not_found() {
        let root = scratch("show-missing");
        std::fs::create_dir_all(root.join("staging/closed")).expect("建 closed");
        let e = cmd_staging(&args(&root, &["--show", "nope.md"])).expect_err("必须报错");
        match e {
            EE::NotFound(m) => assert!(m.contains("nope.md"), "没说是哪个名字: {m}"),
            other => panic!("该是 NotFound，实际 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 没有归档子目录的目录，提示里不许提 `closed/`。
    ///
    /// `review/` 下并没有 `closed/`。原文案是给 `staging` 写的，`note_dir` 被两者
    /// 共用，于是 `review --show 错名` 会建议读者去一个不存在的地方找——那种提示
    /// 比没说更坏，读者会以为自己漏看了什么。
    #[test]
    fn missing_hint_does_not_name_a_directory_that_is_absent() {
        let root = scratch("hint");
        // staging 有 closed/：提示可以提它。
        std::fs::create_dir_all(root.join("staging/closed")).expect("建 closed");
        let e = cmd_staging(&args(&root, &["--show", "nope.md"])).expect_err("必须报错");
        assert!(format!("{e:?}").contains("closed/"), "staging 该提到 closed/: {e:?}");

        // review 没有：提示就该闭嘴。
        std::fs::create_dir_all(root.join("review")).expect("建 review");
        let e = cmd_review(&args(&root, &["--show", "nope.md"])).expect_err("必须报错");
        assert!(!format!("{e:?}").contains("closed/"), "review 下没有 closed/ 却提了它: {e:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}


/// 记录路径 → 短号；没有短号时退回原路径。
///
/// 退回而不是显示空：新写入的记录可能还没分到号（分配表在写命令收尾时才
/// 更新），此时显示原路径能让人看到它确实存在，而不是以为丢了。
fn short_path(nums: &crate::num::Numbers, root: &std::path::Path, p: &std::path::Path) -> String {
    let relp = rel(root, p);
    let id = relp.rsplit('/').next().unwrap_or(&relp).trim_end_matches(".md");
    match nums.num_of(id) {
        Some(n) => n.to_string(),
        None => relp,
    }
}
