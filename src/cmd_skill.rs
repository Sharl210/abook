//! Skill conventions and candidate storage.
//!
//! These rules used to live in the system prompt, where they were paid for on
//! every turn even though a skill is created or fed back once in a while. They
//! belong here instead: one command prints them, and a prompt only has to say
//! "run `abook skill` when the task touches skills".
//!
//! Layout, deliberately beside the memory book rather than inside it — a skill
//! and a memory record are different things and should not share a lifecycle:
//!
//!   ~/AgentSkills/<name>/SKILL.md                the skill itself
//!   ~/AgentSkills/<name>/candidates/<stamp>-<slug>.md   pending feedback
//!   ~/AgentSkills/<name>/decisions.log           what was accepted and why
//!
//! The tool owns storage, naming, the approval gate and the inventory. The
//! merging and rewriting stay with the model: a machine cannot decide whether
//! two pieces of advice belong in one paragraph.

use crate::util::*;
use std::fs;
use std::path::{Path, PathBuf};

/// The whole convention set, printed by `abook skill`.
const CONVENTIONS: &str = r#"# Skill 约定

Skill 是可复用的任务说明集合。下面五节是全部约定，逐条执行。

## 一、先判要不要新建

创建、反哺或更新任何 Skill 之前，先查已装载的 Skill 元数据：

- 有适用的 → `USE_EXISTING`，直接用，不新建。
- 大方向适用、只是要补本项目的适配 → `ADAPT_TASK_TO_SKILL`，改现有那一份。
- 确实无可复用 → 才新建。

新建一率重复造轮子。判断依据是"这个任务已被某份 Skill 覆盖到什么程度"，
不是"这份 Skill 名字对不对"。

## 二、自包含

Skill 必须独立自包含，单独拿出来就能工作。

- 正文、示例、默认步骤、配置、脚本、模板、数据、说明、测试所需内容，全部随 Skill 一并交付。
- 确需外部资料时，先复制到该 Skill 自己的引用目录，再把路径、版本、用途、读取方式写进 Skill。
- 禁运行时依赖外部文件、共享目录、共享缓存、本机环境或某次会话的状态。
- 单 Skill 存在也必须可独立工作。不为省空间而共享文件、公共引用，或复用另一 Skill 的内容。
- 默认禁 Skill 间相互引用、依赖、调用、继承、组合或隐式前置。仅用户对具体关系明确授权时可例外，
  且须把依赖对象、范围、原因、版本和失效处置写进当前 Skill，不得把例外传播给其他 Skill。

创建、反哺、修改前先查独立边界；完成后实读候选正文、引用目录与默认步骤逐条核验：
发现外部路径、共享文件、其他 Skill 名或路径、跨 Skill 调用、未内置资料、"参见/依赖/后续补充"
这类占位、或单独运行缺项，即判失败，须剥离、内置、复制或重建后复验。未过禁交付。

## 三、渐进披露

"精简头部"只指 AI 默认加载的入口部分，不是删知识。

- 头部保留：名称、触发、职责、最短步骤、输出与完成条件、必要导航。
- 知识总量、覆盖、验证、独立自包含与用户锁定要求一律不得删减。
- 头部中不宜默认预加载的内容，可拆分到该 Skill 自身的章节、文件或子目录。
  用引用文件夹时，它视为 Skill 内置知识库，文件数量不设限且鼓励细分：
  每单元只负责一个小方向，清晰命名、分层索引、用相对路径。
- 导航须写明各部分主题、何时读取、必要顺序或依赖。
- 单个参考文件超 100 行时，头部须自带目录；不足 100 行不强制。
- 运行时按当前任务渐进读取，只按需读相关部分，禁一次加载全部或无关内容。
- 组织上少食多餐：细分多份但保持条理，禁合并到造成混乱或过量加载。

交付前核验：导航可达、主题职责不重叠、无悬空引用、无重复内容、无无关默认加载，
整体仍可独立运行。

## 四、交付门

候选交付前须同时满足四条，任一不过就内置缺失资料、复制到自身合适位置、剥离交叉依赖、
重分章节后重验，禁用"后续再补""使用时自备"或口头声明替代：

1. 复用与边界判定已完成（第一节）。
2. 记忆机制隔离：记忆机制的入口、路由、开关、门、令牌、记录格式、写入、晋升、暂存、召回
   及其术语、路径、步骤，禁入 Skill 正文、示例与默认步骤；Skill 须不依赖记忆机制即可独立工作。
   唯一例外是这份 Skill 本身就以记忆机制为对象（专为它而写），此时须在 Skill 内显式声明该依赖，
   且禁把依赖再传播给其他 Skill。
3. 独立自包含（第二节）。
4. 渐进披露（第三节）。

校验证据须是候选正文、拆分内容、默认步骤、导航、按需读取路径、超 100 行文件的头部目录
和单 Skill 运行条件的实际逐条核验，禁用脚本命中计数代替。

## 五、命令

  abook skill                     打印本约定
  abook skill path                打印 Skill 根目录
  abook skill list                列出全部 Skill，含正式文件与待决候选数
  abook skill show <名>           打印某 Skill 的 SKILL.md
  abook skill new <名>            建骨架（SKILL.md + candidates/ + decisions.log）
  abook skill candidate <名> --title T --body-file F
                                  为某 Skill 落一条反哺候选
  abook skill candidates <名>     列出该 Skill 的候选
  abook skill read <名> <候选>    读候选全文
  abook skill decide <名> <候选> accept|reject|hold --approve
                                  记录决定；accept 之后由模型改写 SKILL.md
  abook skill lint <名>           自包含性检查：外部路径、跨 Skill 引用、占位词

决定与落盘是两件事：`decide accept` 只记账，SKILL.md 的合并改写由模型完成，
工具不代做融合——机器无法判断两段建议该不该并成一段。

根目录默认 ~/AgentSkills，用 --root 或 ABOOK_SKILLS 覆盖。
"#;

fn skills_root(a: &Args) -> PathBuf {
    if let Some(r) = a.get("root") {
        if !r.is_empty() {
            return expand_tilde(r);
        }
    }
    let e = env_or("ABOOK_SKILLS", "");
    if !e.is_empty() {
        return expand_tilde(&e);
    }
    // 默认根要认实际存在的那个。
    //
    // 只认 `~/AgentSkills` 的后果：这个目录是空的（或者根本不存在），而真实的
    // skill 在 `~/.agents/skills`，于是 `abook skill list` 报"共 0 个"。
    // 0 个是个**合法答案**，所以没有人会去怀疑路径错了——它看起来就是"你没有
    // 装 skill"。这比报错更糟：报错会让人去查，报 0 只会让人以为事实如此。
    //
    // 三个候选按优先级排，取第一个存在的；都不存在时仍返回旧默认值（首次用
    // `skill new` 时由它自己建目录）。
    let home = home_dir();
    for cand in [
        home.join(".agents").join("skills"),
        home.join(".config").join("agents").join("skills"),
    ] {
        if cand.is_dir() {
            return cand;
        }
    }
    home.join("AgentSkills")
}

fn need_name(a: &Args) -> R<String> {
    match a.pos.first() {
        Some(n) if !n.trim().is_empty() => Ok(sanitize(n)),
        _ => usage("需要 Skill 名，例如 `abook skill new my-skill`"),
    }
}

/// Skill names become directory names. Keep them boring so a stray slash or a
/// leading dot cannot move the write outside the root.
fn sanitize(n: &str) -> String {
    let mut out = String::new();
    for c in n.chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
            out.push(c);
        } else if c.is_whitespace() {
            out.push('-');
        }
    }
    while out.starts_with('.') {
        out.remove(0);
    }
    if out.is_empty() {
        out.push_str("skill");
    }
    out
}

fn skill_dir(a: &Args, name: &str) -> PathBuf {
    skills_root(a).join(name)
}

fn count_md(dir: &Path) -> usize {
    match fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| {
                let p = e.path();
                p.is_file() && p.extension().map(|x| x == "md").unwrap_or(false)
            })
            .count(),
        Err(_) => 0,
    }
}

pub fn cmd_skill(a: &Args) -> R<i32> {
    let sub = a.pos.first().cloned().unwrap_or_default();
    // `abook skill` and `abook skill guide` are the same thing: the convention
    // text is the whole point of the command, so the bare form must print it
    // rather than a usage error.
    match sub.as_str() {
        "" | "guide" | "help" => {
            // `--json` must actually be JSON. It used to print the convention
            // prose and ignore the flag, so a caller that asked for JSON parsed
            // nothing and believed it had data -- the same defect class as the
            // other silently-ignored flags this tool has already cleaned up.
            if a.has("json") {
                crate::say_line!(
                    "{{\"command\":\"skill\",\"subcommand\":\"guide\",\"text\":{}}}",
                    crate::util::jstr(CONVENTIONS)
                );
            } else {
                for line in CONVENTIONS.lines() {
                    crate::say_line!("{line}");
                }
            }
            Ok(0)
        }
        "path" => {
            crate::say_line!("{}", skills_root(a).display().to_string());
            Ok(0)
        }
        "list" => {
            let root = skills_root(a);
            if !root.is_dir() {
                crate::say_line!("{} 还不存在，用 `abook skill new <名>` 建第一份", root.display());
                return Ok(0);
            }
            // (名称, 有无 SKILL.md, 候选数, 字节, 行数, 最近改动)
            let mut rows: Vec<(String, usize, usize, u64, usize, i64)> = Vec::new();
            if let Ok(rd) = fs::read_dir(&root) {
                for e in rd.flatten() {
                    let p = e.path();
                    if !p.is_dir() {
                        continue;
                    }
                    let name = p
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if name.starts_with('.') {
                        continue;
                    }
                    let have = if p.join("SKILL.md").is_file() { 1 } else { 0 };
                    // 体积与行数按 SKILL.md 本体算——那才是读者真正要打开的文件；
                    // 整个目录的字节数会把 references/ 里的大附件算进来，读者据此
                    // 判断"这篇长不长"就会错。
                    let skill_md = p.join("SKILL.md");
                    let (by, ln) = match std::fs::read(&skill_md) {
                        Ok(b) => (b.len() as u64, b.iter().filter(|c| **c == b'\n').count()),
                        Err(_) => (0, 0),
                    };
                    let ts = std::fs::metadata(&skill_md)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    rows.push((name, have, count_md(&p.join("candidates")), by, ln, ts));
                }
            }
            rows.sort();
            if a.has("json") {
                let arr = rows
                    .iter()
                    .map(|(n, h, c, by, ln, ts)| {
                        format!(
                            "{{\"name\":{},\"has_skill_md\":{},\"candidates\":{},\"bytes\":{by},\"lines\":{ln},\"updated\":{ts},\"when\":{}}}",
                            jstr(n),
                            h == &1,
                            c,
                            jstr(&crate::util::short_when(*ts))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                crate::say_line!("{{\"root\":{},\"skills\":[{}]}}", jstr(&root.display().to_string()), arr);
                return Ok(0);
            }
            crate::say_line!("{}  共 {} 个", root.display(), rows.len());
            for (n, h, c, by, ln, ts) in &rows {
                let mark = if *h == 1 { "有SKILL.md" } else { "缺SKILL.md" };
                crate::say_line!(
                    "  {}  {n:<26} {mark}  候选 {c}",
                    crate::util::row_meta(*ts, Some(*by), Some(*ln))
                );
            }
            Ok(0)
        }
        "show" => {
            let name = match a.pos.get(1) {
                Some(n) => sanitize(n),
                None => return usage("需要 Skill 名"),
            };
            let p = skill_dir(a, &name).join("SKILL.md");
            if !p.is_file() {
                return usage(format!("没有 {} ：{} 不存在", name, p.display()));
            }
            let txt = read_str(&p)?;
            for line in txt.lines() {
                crate::say_line!("{line}");
            }
            Ok(0)
        }
        "new" => {
            let name = match a.pos.get(1) {
                Some(n) => sanitize(n),
                None => return usage("需要 Skill 名，例如 `abook skill new my-skill`"),
            };
            let dir = skill_dir(a, &name);
            if dir.join("SKILL.md").is_file() && !a.has("force") {
                return blocked(format!("{} 已存在；确要重建加 --force", dir.display()));
            }
            if a.has("dry-run") {
                crate::say_line!("[dry-run] 将创建 {}", dir.display());
                return Ok(0);
            }
            ensure_dir(&dir)?;
            ensure_dir(&dir.join("candidates"))?;
            let st = stamps();
            let title = opt(a, "title");
            let title = if title.is_empty() { name.clone() } else { title };
            let skeleton = format!(
                "# {title}\n\n\
                 > 骨架由 `abook skill new` 生成。按 `abook skill` 的约定补全后再用。\n\n\
                 ## 何时用\n\n（触发条件：什么样的请求该用这份 Skill）\n\n\
                 ## 职责\n\n（这份 Skill 负责什么，不负责什么）\n\n\
                 ## 最短步骤\n\n1. \n2. \n\n\
                 ## 输出与完成条件\n\n（交付什么形态，怎样算做完）\n\n\
                 ## 参考\n\n（本 Skill 自身的引用文件；超 100 行时在此列出目录）\n"
            );
            let p = dir.join("SKILL.md");
            write_atomic(&p, &skeleton)?;
            let back = read_str(&p)?;
            if back.trim().is_empty() {
                return integrity("写入复读校验失败");
            }
            let log = dir.join("decisions.log");
            if !log.is_file() {
                write_atomic(
                    &log,
                    &format!("# {name} 决定记录\n# 格式: <时间> <候选> <accept|reject|hold> <理由>\n"),
                )?;
            }
            let _ = st;
            crate::say_line!("{}", p.display().to_string());
            crate::say_line!("下一步：按 `abook skill` 的约定补全正文，再 `abook skill lint {name}`");
            Ok(0)
        }
        "candidate" => {
            let name = match a.pos.get(1) {
                Some(n) => sanitize(n),
                None => return usage("需要 Skill 名"),
            };
            let title = opt(a, "title");
            if title.is_empty() {
                return usage("需要 --title");
            }
            let body = match a.get("body-file") {
                Some(f) => read_str(&expand_tilde(f))?,
                None => match a.get("body") {
                    Some(b) => b.to_string(),
                    None => return usage("需要 --body-file 或 --body"),
                },
            };
            if body.trim().is_empty() {
                return usage("候选正文不能为空");
            }
            let dir = skill_dir(a, &name);
            if !dir.is_dir() {
                return usage(format!("{} 还不存在，先 `abook skill new {}`", dir.display(), name));
            }
            let cdir = dir.join("candidates");
            // 预览不建目录：对还没建过候选的 Skill 跑一次 `skill candidate
            // --dry-run`，原来会留下一个空的 `candidates/` 目录。
            if !a.has("dry-run") {
                ensure_dir(&cdir)?;
            }
            let st = stamps();
            let slug = slugify(&title);
            // 候选文件名不能只靠秒级时间戳 + slug 定。
            //
            // 同一秒内用同一标题写两条候选（`--force` 或两条命令并发）会算出
            // 同一个路径，第二条覆盖第一条——而 `write_atomic` 是成功的，
            // 退出码 0，第一条候选就这么没了，没有任何提示。
            //
            // 加一个内容哈希后缀：内容不同则路径必然不同，内容相同则覆盖是
            // 幂等而不是丢失。
            let mut hh: u64 = 0xcbf2_9ce4_8422_2325;
            for b in title.as_bytes().iter().chain(body.as_bytes()) {
                hh ^= *b as u64;
                hh = hh.wrapping_mul(0x1000_0000_01b3);
            }
            // 用全部 64 位，不截到 32 位。
            //
            // 截断到低 32 位后，生日碰撞在几十万条量级就能撞上，而实测已经
            // 撞出来了：两条不同正文的候选算出同一个 `008b1809`。撞上之后
            // 第二条把第一条覆盖掉，退出码 0，第一条凭空消失。
            //
            // 即便 64 位，也不把"哈希不同即路径不同"当成保证——下面还有一层
            // 内容比对：路径已存在且内容不同，就换一个序号，绝不覆盖。
            let mut fname = format!("{}-{slug}-{hh:016x}.md", st.compact);
            let mut p = cdir.join(&fname);
            let same = |q: &std::path::Path| -> bool {
                std::fs::read_to_string(q)
                    .map(|t| t == body)
                    .unwrap_or(false)
            };
            if p.exists() {
                if same(&p) {
                    // 同一条候选重复写入：幂等，不是丢失。
                    if !a.has("force") {
                        return blocked(format!("{} 已存在且内容相同", p.display()));
                    }
                } else {
                    // 不同内容撞了同一个名字：换序号，绝不覆盖已有的那条。
                    let mut n = 2;
                    loop {
                        fname = format!("{}-{slug}-{hh:016x}-{n}.md", st.compact);
                        p = cdir.join(&fname);
                        if !p.exists() {
                            break;
                        }
                        if same(&p) {
                            break;
                        }
                        n += 1;
                        if n > 999 {
                            return blocked("候选文件名分配失败：同名不同内容过多");
                        }
                    }
                }
            }
            let doc = format!(
                "TITLE={title}\nTIME={}\nSTATUS=PENDING\nSKILL={name}\n\n{body}\n",
                st.iso_z
            );
            if a.has("dry-run") {
                crate::say_line!("[dry-run] 将写入 {}", p.display());
                return Ok(0);
            }
            write_atomic(&p, &doc)?;
            let back = read_str(&p)?;
            if !back.contains(&title) {
                return integrity("写入复读校验失败");
            }
            crate::say_line!("{}", p.display().to_string());
            Ok(0)
        }
        "candidates" => {
            let name = match a.pos.get(1) {
                Some(n) => sanitize(n),
                None => return usage("需要 Skill 名"),
            };
            let cdir = skill_dir(a, &name).join("candidates");
            if !cdir.is_dir() {
                crate::say_line!("{} 还没有候选", name);
                return Ok(0);
            }
            let mut rows: Vec<(String, String, String)> = Vec::new();
            if let Ok(rd) = fs::read_dir(&cdir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if !p.is_file() {
                        continue;
                    }
                    let txt = read_str(&p).unwrap_or_default();
                    let want = |k: &str| -> String {
                        txt.lines()
                            .find(|l| l.starts_with(&format!("{k}=")))
                            .map(|l| l[k.len() + 1..].to_string())
                            .unwrap_or_default()
                    };
                    rows.push((
                        p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                        want("STATUS"),
                        want("TITLE"),
                    ));
                }
            }
            rows.sort();
            crate::say_line!("{}  候选 {} 条", name, rows.len());
            for (f, st, t) in &rows {
                crate::say_line!("  {st:<8} {f}\n           {t}");
            }
            Ok(0)
        }
        "read" => {
            let name = match a.pos.get(1) {
                Some(n) => sanitize(n),
                None => return usage("需要 Skill 名"),
            };
            let cand = match a.pos.get(2) {
                Some(c) => cand_name(c)?,
                None => return usage("需要候选文件名"),
            };
            let p = skill_dir(a, &name).join("candidates").join(&cand);
            if !p.is_file() {
                return usage(format!("没有这个候选：{}", p.display()));
            }
            let txt = read_str(&p)?;
            for line in txt.lines() {
                crate::say_line!("{line}");
            }
            Ok(0)
        }
        "decide" => {
            let name = match a.pos.get(1) {
                Some(n) => sanitize(n),
                None => return usage("需要 Skill 名"),
            };
            let cand = match a.pos.get(2) {
                Some(c) => cand_name(c)?,
                None => return usage("需要候选文件名"),
            };
            let decision = match a.pos.get(3).map(|s| s.as_str()) {
                Some("accept") => "accept",
                Some("reject") => "reject",
                Some("hold") => "hold",
                _ => return usage("决定须为 accept / reject / hold"),
            };
            if decision == "accept" && !a.has("approve") {
                return blocked("接受候选需要用户明确批准：确认后加 --approve");
            }
            let dir = skill_dir(a, &name);
            let p = dir.join("candidates").join(&cand);
            if !p.is_file() {
                return usage(format!("没有这个候选：{}", p.display()));
            }
            let txt = read_str(&p)?;
            let mut out = String::new();
            for line in txt.lines() {
                if line.starts_with("STATUS=") {
                    out.push_str(&format!("STATUS={}\n", decision.to_uppercase()));
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            let st = stamps();
            let reason = opt(a, "reason");
            if a.has("dry-run") {
                crate::say_line!("[dry-run] 将把 {} 标为 {}", cand, decision);
                return Ok(0);
            }
            // 先写日志，再改候选。
            //
            // 原来顺序反过来：候选的 STATUS 先落盘，然后写 decisions.log。日志
            // 目录一旦不可写（权限、磁盘满），命令以 rc=6 失败——但候选已经被
            // 改成 REJECT，而决策记录不存在。失败了却已经生效，且没有留下它
            // 生效过的痕迹。
            //
            // 反过来先写日志：日志写不成就在动候选之前失败，数据不变；日志写成
            // 之后候选写失败，则日志里有一条指向未生效决定的记录，这是可以发
            // 现并纠正的偏差，比反向的"已生效但无记录"轻得多。
            let log = dir.join("decisions.log");
            let prev = if log.is_file() { read_str(&log)? } else { String::new() };
            write_atomic(&log, &format!("{prev}{} {cand} {decision} {reason}\n", st.iso_z))?;
            write_atomic(&p, &out)?;
            crate::say_line!("{cand} → {}", decision.to_uppercase());
            if decision == "accept" {
                crate::say_line!("记住：SKILL.md 的合并改写由你完成，工具不代做融合；改完跑 `abook skill lint {name}`");
            }
            Ok(0)
        }
        "lint" => {
            let name = match a.pos.get(1) {
                Some(n) => sanitize(n),
                None => return usage("需要 Skill 名"),
            };
            let dir = skill_dir(a, &name);
            let p = dir.join("SKILL.md");
            if !p.is_file() {
                return usage(format!("{} 不存在", p.display()));
            }
            let txt = read_str(&p)?;
            // 检查结果分两类：确定是问题的，和"需要人看一眼前提"的。
    //
    // 绝对路径这一项现在会把文档里提到的路径也报出来——`/opt/s_bin 在 PATH`
    // 这类是**前置条件说明**，不是"运行时要读本机某个文件"。前者写在文档里是
    // 有用的（告诉使用者要装什么），后者才是破坏自包含的。
    //
    // 靠词法分不开这两者，所以不硬猜：确定项照旧判不通过，存疑项单独列出并
    // 说明该怎么自己判断。把正确答案判成错误答案，比漏检更糟——人会开始绕过
    // 这条检查，然后真问题也一并漏掉。
    let mut problems: Vec<String> = Vec::new();
    let mut advisory: Vec<String> = Vec::new();
            for (n, line) in txt.lines().enumerate() {
                let ln = n + 1;
                let t = line.trim();
                // 绝对路径：原来只认 `/home/` 和 `C:\`，`/tmp/`、`/root/`、
                // 盘符任意位置都能漏过去。判据应该是"任何以 / 或 X:\ 开头的
                // 路径片段"，而不是列举几个我想到过的前缀。
                if has_abs_path(t) {
                    advisory.push(format!("行{ln}: 出现了绝对路径（若为运行时要读的文件则不合规；仅作前置条件说明可忽略）: {}", cut(t, 60)));
                }
                // 相对逃逸：`../` 会读到 Skill 目录之外，同样破坏自包含。
                if has_relative_escape(t) {
                    problems.push(format!("行{ln}: 相对路径逃出 Skill 目录: {}", cut(t, 60)));
                }
                if t.contains("参见") || t.contains("见另一") || t.contains("依赖其它 Skill")
                    || t.contains("后续补充")
                {
                    problems.push(format!("行{ln}: 占位或跨 Skill 引用: {}", cut(t, 60)));
                }
                // 记忆机制术语。原来写成 `A || B || C && D`，而 && 比 || 结合
                // 更紧，等价于 `A || B || (C && D)`——只要出现 ABOOK_ 就报警，
                // 而「记忆库」单独出现反而不报。这里改成显式括号 + 词表。
                if t.contains("~/AgentsBook")
                    || t.contains("ABOOK_")
                    || t.contains("ABOOK_ROOT")
                    || (t.contains("记忆库") && (t.contains("GATE") || t.contains("门")))
                    || t.contains("MEMORY_ROUTE")
                    || t.contains("AGENTSBOOK_V2")
                {
                    problems.push(format!("行{ln}: 疑似记忆机制术语（Skill 须能独立工作）: {}", cut(t, 60)));
                }
                // 环境变量展开出来的路径，同样是"运行时依赖本机环境"。
                //
                // 原来的判据只看字面路径（`/` 或 `X:\` 开头），而
                // `$HOME/.config/abook/settings.toml` 这种写法的第一个字符是
                // `$`，斜杠前面紧挨着 `E` 之类字母，`boundary` 判据不成立，
                // 于是一路走过去也不报警。`%LOCALAPPDATA%` 同理。
                //
                // 判据应该是"这个路径是不是由本机环境变量决定的"，而不是
                // "它长得像不像一个路径"。凡是靠 `$VAR` / `%VAR%` / `${VAR}`
                // 展开才能确定位置的，都破坏了 Skill 的可移植性——换台机器、
                // 换个人用，路径就变了。
                if has_env_path(t) {
                    problems.push(format!(
                        "行{ln}: 路径依赖本机环境变量（Skill 须自带依赖）: {}",
                        cut(t, 60)
                    ));
                }
            }
            // 跨 Skill 引用：拿同级目录里真实存在的 Skill 名去比对，而不是只认
            // 几个我写死的短语。引用一个确实存在的兄弟 Skill 是最常见的越界方式。
            if let Some(parent) = dir.parent() {
                if let Ok(rd) = fs::read_dir(parent) {
                    let siblings: Vec<String> = rd
                        .flatten()
                        .filter(|e| e.path().is_dir())
                        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                        // 排除自己。`design-patterns-advisor` 里出现 `design`
                        // 是它自己名字的一部分，不是"引用了另一个 Skill"。
                        // 名字互相包含在真实库里很常见（`design` / `design-tokens`
                        // / `design-review` / `design-patterns-advisor`），不排除
                        // 自己的话，每个这样命名的 Skill 都必然报一片。
                        .filter(|s| {
                            s != &name && !name.contains(s.as_str()) && !s.starts_with('.')
                        })
                        .collect();
                    for (n, line) in txt.lines().enumerate() {
                        let t = line.trim();
                        for sib in &siblings {
                            if mentions_skill_ref(t, sib) {
                                problems.push(format!(
                                    "行{}: 引用了另一个 Skill `{sib}`（Skill 之间不互相依赖）: {}",
                                    n + 1,
                                    cut(t, 60)
                                ));
                            }
                        }
                    }
                }
            }
            // 递归检查 Skill 目录下的**所有**文本文件。
            //
            // 原来只扫 SKILL.md 和目录的直接子文件。Skill 的正文可以拆到
            // `references/`、脚本、模板里，而自包含的要求覆盖整个 Skill——
            // 只查门面的话，把 `/tmp/private/config.json`、`../shared/data.yaml`、
            // 记忆机制术语、兄弟 Skill 名全塞进 `references/deep.md`，lint 依旧
            // 返回 `problems:[]`、`ok:true`。检查范围小于被检查对象，等于没查。
            let mut files: Vec<std::path::PathBuf> = Vec::new();
            collect_text_files(&dir, &mut files, 0);
            files.sort();
            for q in &files {
                let disp = q
                    .strip_prefix(&dir)
                    .unwrap_or(q)
                    .to_string_lossy()
                    .to_string();
                let sub = match read_str(q) {
                    Ok(t) => t,
                    // 二进制文件（图片、pdf、压缩包）跳过；它们不是"引用"的载体。
                    Err(_) => continue,
                };
                if q.file_name().map(|s| s == "SKILL.md").unwrap_or(false) {
                    continue;
                }
                // 逐行套用与 SKILL.md 相同的判据
                for (n, line) in sub.lines().enumerate() {
                    let t = line.trim();
                    if has_abs_path(t) {
                        problems.push(format!(
                            "{disp} 行{}: 绝对路径（Skill 须自带依赖）: {}",
                            n + 1,
                            cut(t, 50)
                        ));
                    }
                    if has_relative_escape(t) {
                        problems.push(format!(
                            "{disp} 行{}: 相对路径逃出 Skill 目录: {}",
                            n + 1,
                            cut(t, 50)
                        ));
                    }
                    if has_env_path(t) {
                        problems.push(format!(
                            "{disp} 行{}: 路径依赖本机环境变量（Skill 须自带依赖）: {}",
                            n + 1,
                            cut(t, 50)
                        ));
                    }
                    if t.contains("~/AgentsBook")
                        || t.contains("ABOOK_")
                        || t.contains("MEMORY_ROUTE")
                        || t.contains("AGENTSBOOK_V2")
                        || (t.contains("记忆库") && (t.contains("GATE") || t.contains("门")))
                    {
                        problems.push(format!(
                            "{disp} 行{}: 疑似记忆机制术语（Skill 须能独立工作）: {}",
                            n + 1,
                            cut(t, 50)
                        ));
                    }
                    if let Some(parent) = dir.parent() {
                        if let Ok(rd) = fs::read_dir(parent) {
                            for e in rd.flatten() {
                                if !e.path().is_dir() {
                                    continue;
                                }
                                let sib = e.file_name().to_string_lossy().to_string();
                                if sib != name
                                    && !name.contains(sib.as_str())
                                    && !sib.starts_with('.')
                                    && mentions_skill_ref(t, &sib)
                                {
                                    problems.push(format!(
                                        "{disp} 行{}: 引用了另一个 Skill `{sib}`: {}",
                                        n + 1,
                                        cut(t, 50)
                                    ));
                                }
                            }
                        }
                    }
                }
                if sub.lines().count() > 100 {
                    problems.push(format!("{disp} 超 100 行，须在头部给出它的目录"));
                }
            }
            if a.has("json") {
                let arr = problems.iter().map(|p| jstr(p)).collect::<Vec<_>>().join(",");
                let adv = advisory.iter().map(|p| jstr(p)).collect::<Vec<_>>().join(",");
                crate::say_line!(
                    "{{\"skill\":{},\"problems\":[{}],\"advisory\":[{}],\"ok\":{}}}",
                    jstr(&name),
                    arr,
                    adv,
                    problems.is_empty()
                );
                return Ok(0);
            }
            // 存疑项永远打在确定项之后、并且明说"这些不算不通过"。
            // 混在一起数总量会让人以为 Skill 烂了 159 处，而其实绝大部分是
            // 文档里正常出现的路径。
            for x in &advisory {
                crate::say_line!("  · {x}");
            }
            if !advisory.is_empty() {
                crate::say_line!(
                    "（以上 {} 条需自行判断：绝对路径作为前置条件写在文档里是正常的，作为运行时依赖才不合规）",
                    advisory.len()
                );
            }
            if problems.is_empty() {
                crate::say_line!("{} 通过：无外部依赖、无跨 Skill 引用、无占位词", name);
                Ok(0)
            } else {
                crate::say_line!("{} 有问题 {} 处：", name, problems.len());
                for x in &problems {
                    crate::say_line!("  {x}");
                }
                integrity("Skill 未过自包含检查")
            }
        }
        other => usage(format!(
            "skill 没有子命令 `{other}`；可用：guide list show new candidate candidates read decide lint path"
        )),
    }
}

/// Reject anything that is not a plain file name inside the candidates directory.
///
/// `skill read`/`decide` used the raw argument as a path component, so
/// `abook skill decide x ../../../../tmp/victim reject` escaped the candidates
/// directory and rewrote whatever file the caller named -- including files
/// outside the skills tree entirely. A candidate is identified by its file name,
/// nothing about that needs to accept a path.
/// 行内是否出现绝对路径片段。
///
/// 判据按形态而不按前缀列举：`/xxx` 前是行首或空白/引号/括号，或 `C:\` 这类
/// 盘符。列举前缀的做法永远漏（曾经只认 `/home/`，于是 `/tmp` 畅通无阻）。
/// 递归收集 Skill 目录下的文本文件。
///
/// 跳过 `.git`、`node_modules` 这类明显不是内容的地方；深度上限 8 层，
/// 避免软链接成环时无限下钻。
fn collect_text_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
    if depth > 8 {
        return;
    }
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for e in rd.flatten() {
        let p = e.path();
        let nm = e.file_name().to_string_lossy().to_string();
        // 跳过的东西分两类，理由不同：
        //
        //   - 构建/缓存类（`.git`、`node_modules`、`target`、`__pycache__`）：
        //     不是 Skill 的正文，扫它们只会淹掉真正的问题。
        //   - 第三方自带代码（`vendor`、`third_party`、`external`）：
        //     这些**恰恰是自包含的正确做法**——把依赖复制进 Skill 目录，
        //     总比运行时去别处取好。里面的 `"/fake/bin/repo"`、`~/.repoconfig`
        //     是别人测试代码里的字符串常量，不是本 Skill 的外部依赖。
        //
        // 不排除第二类的后果：一个正确打包了 vendor 的 Skill，lint 报几百条
        // "绝对路径""超 100 行须给目录"，退 5 判不通过。**检查把正确答案判成
        // 错误答案**，比漏检更糟——人会开始绕过这条检查，然后真问题也一并漏掉。
        if nm.starts_with('.')
            || matches!(
                nm.as_str(),
                "node_modules" | "target" | "__pycache__" | "vendor" | "third_party" | "external"
            )
        {
            continue;
        }
        let md = match fs::metadata(&p) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if md.is_dir() {
            collect_text_files(&p, out, depth + 1);
        } else if md.is_file() {
            out.push(p);
        }
    }
}

/// 这一行是不是真的在引用另一个 Skill（而不只是碰巧出现这几个字符）。
///
/// 光有整词边界还不够：真实库里 `design` 会在 `refactoringguru.cn/design-patterns/`
/// 这种 URL 里、`.design/<slug>/` 这种目录约定里大量出现；`pro` 会在
/// `professional`、`prompt` 里出现。这些都不是"引用另一个 Skill"。
///
/// 所以判据落在**引用的形态**上，而不是字符是否相邻：
///   1. `xxx/SKILL.md`        目录引用，最明确
///   2. `../xxx`              跳到兄弟目录（相对逃逸已单独报，这里也算引用）
///   3. 反引号或引号包住的 `xxx`  显式点名
///   4. 同一行提到 skill / 技能  在讲 Skill 的句子里点它的名
///
/// 四条之外一律不算。宁可漏掉偶发的隐式引用，也不要把整库文档链接标红——
/// 一份 18668 条的报告没人会读，那等于没有这个检查。
fn mentions_skill_ref(line: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    if line.contains(&format!("{name}/SKILL.md")) {
        return true;
    }
    if line.contains(&format!("../{name}")) {
        return true;
    }
    for q in ['`', '"', '\''] {
        if line.contains(&format!("{q}{name}{q}")) {
            return true;
        }
    }
    let lower = line.to_lowercase();
    let talks_skill = lower.contains("skill") || line.contains("技能");
    if talks_skill && mentions_name(line, name) {
        return true;
    }
    false
}

/// 这一行是不是真的提到了另一个 Skill 的名字。
///
/// 原来是 `t.contains(sib)`，纯子串匹配。真实库里有一批极短的 Skill 名：
/// `pro`、`yes`、`dist`。于是 `professional`、`prompt`、`process`、
/// `distribute`、`yes/no` 全被判成"引用了另一个 Skill"——`design-patterns-advisor`
/// 一个文件就报了 1211 处，几乎全是普通英文单词。
///
/// 名字越短越容易撞，所以判据要从"出现过这几个字符"变成"作为一个词出现过"：
/// 前后都不能紧邻字母、数字、下划线。Skill 名本身可以含连字符
/// （`lark-shared`），所以连字符只在**名字内部**算名字的一部分，出现在
/// 名字两端时算边界。
fn mentions_name(line: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let lb: Vec<char> = line.chars().collect();
    let nb: Vec<char> = name.chars().collect();
    if nb.len() > lb.len() {
        return false;
    }
    let is_wordish = |c: char| c.is_alphanumeric() || c == '_';
    for start in 0..=(lb.len() - nb.len()) {
        if lb[start..start + nb.len()] != nb[..] {
            continue;
        }
        let before_ok = start == 0 || !is_wordish(lb[start - 1]);
        let end = start + nb.len();
        let after_ok = end >= lb.len() || !is_wordish(lb[end]);
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

fn has_abs_path(t: &str) -> bool {
    let b: Vec<char> = t.chars().collect();
    let at = |i: usize| b.get(i).copied().unwrap_or(' ');
    // 这个位置是不是落在 URL 里。
    //
    // 判据：往前找到 `://` 就算 URL。`https://gsap.com/docs/v3` 里的两个斜杠
    // 前面紧挨着 `:`，原来的边界判据把 `:` 也算成边界，于是**每一个 URL 都被
    // 报成了"绝对路径"**——真实库 118 个 Skill 里有 116 个中招，这个检查等于
    // 废掉：没人会去读一份把所有文档链接都标红的报告。
    //
    // 斜杠之间有空白、引号、括号就说明已经出了这个片段，不再往后找。
    let in_url = |i: usize| -> bool {
        let mut k = i;
        while k > 0 {
            k -= 1;
            if b[k] == ':' && at(k + 1) == '/' && at(k + 2) == '/' {
                return true;
            }
            if b[k].is_whitespace() || matches!(b[k], '"' | '\'' | '(' | ')' | '[' | ']' | '<' | '>')
            {
                return false;
            }
        }
        false
    };
    // `/x` 只有一段，不是路径，是斜杠命令（`/pua`、`/graphify`、`/design-review`）
    // 或除法。真正的绝对路径至少有 `/a/b` 两段。
    //
    // 不加这一条，所有 slash 命令都会被标成"绝对路径"。
    let segments_from = |i: usize| -> usize {
        let mut n = 0;
        let mut j = i;
        let mut in_seg = false;
        while j < b.len() {
            let c = b[j];
            if c == '/' {
                if in_seg {
                    n += 1;
                }
                in_seg = false;
            } else if c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']' | '>' | '`' | ',') {
                break;
            } else {
                in_seg = true;
            }
            j += 1;
        }
        if in_seg {
            n += 1;
        }
        n
    };
    for i in 0..b.len() {
        if b[i] == '/' {
            if in_url(i) {
                continue;
            }
            // 行首的 `/` 也算绝对路径：多行文本里路径常独占一行，
            // 原来 `i > 0` 直接把它排除了。
            let boundary = if i == 0 {
                true
            } else {
                let prev = b[i - 1];
                prev.is_whitespace()
                    || matches!(prev, '"' | '\'' | '(' | '[' | '=' | '`' | '>')
            };
            if boundary && at(i + 1).is_alphabetic() && segments_from(i) >= 2 {
                return true;
            }
        }
        // 盘符：字母 + `:` + `\` 或 `/`
        //
        // 必须限定在**词首**。`https://` 里那个 `s:` 就是 `s` + `:` + `/`，
        // 形态与盘符完全一样，于是每一个 https 链接都被判成 Windows 路径——
        // 这是上一版最大的误报来源，比"斜杠命令"那条严重得多。
        // 真正的盘符前面不会是字母数字（`C:\Users`、`(C:\`、`磁盘=C:\`）。
        if b[i].is_ascii_alphabetic()
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
            && at(i + 1) == ':'
            && matches!(at(i + 2), '\\' | '/')
        {
            return true;
        }
    }
    false
}

/// 这一行里有没有相对路径逃出 Skill 目录（`../`）。
///
/// 两种情况要排除：
/// - `.../`：省略号后面跟斜杠，是"省略中间层级"的写法，不是上跳一级。
/// - URL 里的 `../`：那是服务端路径，跟本地目录无关。
fn has_relative_escape(t: &str) -> bool {
    let b: Vec<char> = t.chars().collect();
    let at = |i: usize| b.get(i).copied().unwrap_or(' ');
    let mut i = 0;
    while i + 2 < b.len() {
        if b[i] == '.' && b[i + 1] == '.' && b[i + 2] == '/' {
            let prev_is_dot = i > 0 && b[i - 1] == '.';
            // URL 里：往前找 `://`
            let mut k = i;
            let mut in_url = false;
            while k > 0 {
                k -= 1;
                if b[k] == ':' && at(k + 1) == '/' && at(k + 2) == '/' {
                    in_url = true;
                    break;
                }
                if b[k].is_whitespace() || matches!(b[k], '"' | '\'' | '(' | ')' | '[' | ']') {
                    break;
                }
            }
            if !prev_is_dot && !in_url {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// 这一行里的路径是不是靠本机环境变量展开才能确定。
///
/// 认三种写法：`$VAR/...`、`${VAR}/...`、`%VAR%\...`。
///
/// 为什么这算越界：`$HOME/.config/x` 在哪台机器上指向哪儿，取决于那台机器的
/// 环境。Skill 的全部内容应当随 Skill 一起交付、单独运行即可工作，凡是位置
/// 由运行环境决定的引用，换台机器就断了，而交付者看不出来——他本机跑得好好的。
///
/// 判据刻意收窄到"变量后面紧跟路径分隔符"：单纯提一句 `$PATH` 或
/// `export FOO=1` 是正常的 shell 说明，不是外部路径依赖，不该误伤。
/// 真正表示"位置取决于跑在谁的机器上"的变量名。
///
/// 判据从"任何 `$VAR/` 都算"收窄到"宿主环境变量才算"，因为前者的误报率实际上
/// 让这条检查失效：Skill 自己的脚本里写 `S=$(cd "$(dirname "$0")" && pwd)` 之后
/// 用 `$S/scripts/x.sh` 定位自己，这是**最正确的写法**（不依赖调用者的 cwd），
/// 却会被判成"路径依赖本机环境变量"。
///
/// 实测一个正确打包的 Skill 因此报出上百条，退 5 判不通过。检查把正确答案判成
/// 错误答案，人就会绕过这条检查——然后真正依赖 `$HOME/.config` 的地方也一并漏掉。
///
/// 所以只看这些"换台机器就变"的变量。它们要么是家目录，要么是系统临时目录，
/// 要么是 Windows 的用户级路径。
const HOST_ENV_VARS: &[&str] = &[
    "HOME", "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "APPDATA", "LOCALAPPDATA",
    "PROGRAMDATA", "PROGRAMFILES", "SYSTEMROOT", "WINDIR", "TEMP", "TMP", "TMPDIR",
    "USER", "USERNAME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME",
    "XDG_STATE_HOME", "XDG_RUNTIME_DIR", "SHELL",
];

fn has_env_path(t: &str) -> bool {
    let b: Vec<char> = t.chars().collect();
    let at = |i: usize| b.get(i).copied().unwrap_or(' ');
    let is_sep = |c: char| c == '/' || c == '\\';
    let mut i = 0;
    while i < b.len() {
        // `~/` 与 `~\`：家目录相对路径。
        //
        // 它和 `$HOME/` 是同一件事的两种写法：位置取决于运行这个 Skill 的是谁。
        // 原来完全没管，于是 `python3 ~/.claude/skills/design/scripts/cip/...`
        // 一路走过去都不报警——而那正是最常见的"依赖本机既有目录"写法。
        if b[i] == '~' && is_sep(at(i + 1)) {
            return true;
        }
        // $VAR/ 与 ${VAR}/
        if b[i] == '$' {
            let mut j = i + 1;
            if at(j) == '{' {
                j += 1;
                let start = j;
                while at(j).is_alphanumeric() || at(j) == '_' {
                    j += 1;
                }
                let nm: String = b[start..j].iter().collect();
                if j > start
                    && at(j) == '}'
                    && is_sep(at(j + 1))
                    && HOST_ENV_VARS.contains(&nm.as_str())
                {
                    return true;
                }
            } else {
                let start = j;
                while at(j).is_alphanumeric() || at(j) == '_' {
                    j += 1;
                }
                let nm: String = b[start..j].iter().collect();
                if j > start && is_sep(at(j)) && HOST_ENV_VARS.contains(&nm.as_str()) {
                    return true;
                }
            }
        }
        // %VAR% 形态单独出现也算。
        //
        // 与 `$VAR` 不同：Windows 里 `%…%` 包里装的基本都是路径类变量
        // （LOCALAPPDATA、APPDATA、TEMP、USERPROFILE），而且这个写法在
        // Skill 正文里出现，本身就是"照着本机环境去找东西"。
        // shell 说明里不会用它表示普通变量，所以这里不必要求跟分隔符，
        // 放宽了也不会误伤 `export FOO=1` 那类写法。
        if b[i] == '%' {
            let start = i + 1;
            let mut j = start;
            while at(j).is_alphanumeric() || at(j) == '_' {
                j += 1;
            }
            // 名字至少 3 个字符。
            //
            // 不加这条，`old.strftime('%Y-%m-%dT%H:%M:%S%z')` 里的 `%S%` 会被
            // 当成一个 Windows 路径变量（`%S` 后紧跟 `%`），于是每一处 strftime
            // 格式化都被报成"路径依赖本机环境变量"。真实 Skill 的测试脚本里
            // 这种写法很常见，误报量足以让人不再看这份报告。
            //
            // Windows 自带变量最短的是 `%TMP%`（3 个字符），所以 3 是安全下界；
            // 而 strftime 指令全是单字符，正好被挡在外面。
            if j - start >= 3 && at(j) == '%' {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn cand_name(raw: &str) -> R<String> {
    let p = std::path::Path::new(raw);
    let bad = raw.is_empty()
        || raw == "."
        || raw == ".."
        || p.components().count() != 1
        || raw.contains('/')
        || raw.contains('\\')
        || p.is_absolute();
    if bad {
        return usage(format!(
            "候选名必须是文件名字符串，不能含路径：{raw}"
        ));
    }
    Ok(raw.to_string())
}

fn opt(a: &Args, k: &str) -> String {
    a.get(k).unwrap_or("").to_string()
}

fn cut(s: &str, n: usize) -> String {
    let v: Vec<char> = s.chars().collect();
    if v.len() <= n {
        return s.to_string();
    }
    v[..n].iter().collect::<String>() + "…"
}

fn slugify(t: &str) -> String {
    let mut out = String::new();
    for c in t.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if c.is_whitespace() || c == '-' || c == '_' {
            if !out.ends_with('-') {
                out.push('-');
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("cand");
    }
    if out.len() > 40 {
        out.truncate(40);
    }
    out
}

#[cfg(test)]
mod lint_probe_tests {
    #[test]
    fn probe() {
        let cases = [
            ("- 原文链接：https://refactoringguru.cn/design-patterns/catalog", "URL"),
            ("| f | [references/patterns/factory-method.md](./patterns/factory-method.md) | [x](https://refactoringguru.cn/design-patterns/factory-method) |", "MIX"),
            ("python3 ~/.claude/skills/design/scripts/cip/generate.py", "HOME"),
            ("ls -l /dev/kvm", "LOCAL"),
            ("/pua", "SLASH-CMD"),
        ];
        for (s, tag) in cases {
            println!("{tag}: abs={} env={} esc={}", super::has_abs_path(s), super::has_env_path(s), super::has_relative_escape(s));
        }
    }
}
