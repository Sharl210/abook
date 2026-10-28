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

这条原则不是交付前的一次性检查，而是 Skill 的常驻约束：创建时定结构，维护时守结构，
反哺时先想落点，修改与迁移时重校结构。任何一次编辑都要满足本节，写完跑 `abook skill lint`。

### 主入口有预算

`SKILL.md` 是每次被调用时上下文要付的钱，所以它必须小。

- 预算 ≤ 120 行（`lint --max-entry N` 可覆盖，那是给用户锁定的例外，不是默认）。
- 主入口只留：名称、触发、职责、最短步骤、输出与完成条件、导航。
- 超预算就先拆再写，禁"这版先堆着，以后再说"。
- 知识总量、覆盖、验证、独立自包含与用户锁定要求一律不得因拆分而删减。

### 拆分落点：引用文件，不是主入口

- 反哺与维护产生的细节、案例、参数表、排错、变体流程，默认写进 `references/` 专文，
  主入口只加一行导航。只有触发条件、职责边界、最短步骤、输出或完成条件本身变了，
  才动主入口。
- 每份引用文件只管一个小方向，命名说清是什么，用相对路径引用。文件数量不设限，
  鼓励细分；反对的是合并成一篇大文件，不是拆得多。

### 导航即索引

- 每份引用文件都必须在主入口登记过——没登记的文件运行时根本不会被发现，等于不存在。
- 登记过的必须真实存在，禁悬空路径。
- 导航写清：管什么、什么情况下读、有无顺序依赖。
- 单份引用文件超 100 行时，它自己的开头要给目录；主入口不因此变长。

### 运行时按需读

只读当前任务需要的那几份，禁一次加载全部或无关内容。少食多餐：拆细但有条理，
反对合并成混乱，也反对拆到找不着。

交付前核验：主入口在预算内、导航可达、职责不重叠、无悬空引用、无未登记文件、
无重复内容、无无关默认加载，整体仍可独立运行。

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
  abook skill list                列出全部 Skill：入口行数、引用文件数、候选数
  abook skill show <名>           打印某 Skill 的 SKILL.md
  abook skill new <名>            建骨架（SKILL.md + candidates/ + decisions.log）
  abook skill candidate <名> --title T --body-file F
                                  为某 Skill 落一条反哺候选
  abook skill candidates <名>     列出该 Skill 的候选
  abook skill read <名> <候选>    读候选全文
  abook skill decide <名> <候选> accept|reject|hold --approve
                                  记录决定；accept 之后由模型改写 SKILL.md
  abook skill lint <名>           实证检查：自包含 + 主入口预算 + 导航可达
                                  --max-entry N 覆盖 120 行预算（用户锁定例外）

决定与落盘是两件事：`decide accept` 只记账，SKILL.md 的合并改写由模型完成，
工具不代做融合——机器无法判断两段建议该不该并成一段。

技能根自动选定：在 ~/.dsh/skills、~/.agents/skills、~/.config/agents/skills、
~/AgentSkills 里挑"装得最多"的那个（有 SKILL.md 的子目录计数）。为什么不是
"第一个存在的"：`~/.agents/skills` 往往只有一两份实验残留，真实库在
`~/.dsh/skills`；先到先得会让 `list` 报"共 1 个"——这个数字看起来完全正常，
没人会怀疑路径挑错，于是整套工具对着空目录干活。也可用 --root 或
ABOOK_SKILLS 显式指定。
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
    // 候选目录里**装得最多的那个**才算根，不是排在前面那个。
    //
    // 原来取"第一个存在的"，而 `~/.agents/skills` 往往只有一两份（多半是某次
    // 实验留下的），真实库在 `~/.dsh/skills` 里躺着几十份。先到先得的结果是
    // `abook skill list` 报"共 1 个"——这个数字**看起来完全正常**，没人会怀疑
    // 路径挑错了，于是整套 Skill 工具对着空目录干活。0 会让人去查，1 不会。
    //
    // 计数并列时按优先级前者胜；全都没有时返回旧默认值（首次 `new` 自己建）。
    let home = home_dir();
    let mut best: Option<(std::path::PathBuf, usize)> = None;
    for cand in [
        home.join(".dsh").join("skills"),
        home.join(".agents").join("skills"),
        home.join(".config").join("agents").join("skills"),
        home.join("AgentSkills"),
    ] {
        if !cand.is_dir() {
            continue;
        }
        let n = count_skill_dirs(&cand);
        if n == 0 {
            continue;
        }
        if best.as_ref().map(|(_, bn)| n > *bn).unwrap_or(true) {
            best = Some((cand, n));
        }
    }
    if let Some((p, _)) = best {
        return p;
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

/// 数一份 Skill 的引用文件（不含 SKILL.md、候选与决定记录）。
fn count_ref_files(dir: &Path) -> usize {
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    collect_text_files(dir, &mut files, 0);
    files
        .iter()
        .filter(|q| {
            let rel = q
                .strip_prefix(dir)
                .unwrap_or(q)
                .to_string_lossy()
                .replace('\\', "/");
            rel != "SKILL.md" && rel != "decisions.log" && !rel.starts_with("candidates/")
        })
        .count()
}

/// 数一个目录下有几份 Skill（有 SKILL.md 的子目录）。
fn count_skill_dirs(root: &Path) -> usize {
    match fs::read_dir(root) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| {
                let p = e.path();
                p.is_dir() && p.join("SKILL.md").is_file()
            })
            .count(),
        Err(_) => 0,
    }
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
            // (名称, 有无 SKILL.md, 候选数, 字节, 行数, 最近改动, 引用文件数)
            let mut rows: Vec<(String, usize, usize, u64, usize, i64, usize)> = Vec::new();
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
                    // 引用文件数一起报：只报入口行数看不出"有没有拆"，
                    // 两个数字并排才是一张体检表（入口 40 行 + 引用 12 份 是
                    // 健康形态，入口 900 行 + 引用 0 份 一眼就知道没拆）。
                    let refs = count_ref_files(&p);
                    rows.push((name, have, count_md(&p.join("candidates")), by, ln, ts, refs));
                }
            }
            rows.sort();
            if a.has("json") {
                let arr = rows
                    .iter()
                    .map(|(n, h, c, by, ln, ts, rf)| {
                        format!(
                            "{{\"name\":{},\"has_skill_md\":{},\"candidates\":{},\"bytes\":{by},\"lines\":{ln},\"refs\":{rf},\"over_budget\":{},\"updated\":{ts},\"when\":{}}}",
                            jstr(n),
                            h == &1,
                            c,
                            *ln > ENTRY_MAX_LINES,
                            jstr(&crate::util::short_when(*ts))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                crate::say_line!("{{\"root\":{},\"skills\":[{}]}}", jstr(&root.display().to_string()), arr);
                return Ok(0);
            }
            crate::say_line!("{}  共 {} 个", root.display(), rows.len());
            for (n, h, c, by, ln, ts, rf) in &rows {
                let mark = if *h == 1 { "有SKILL.md" } else { "缺SKILL.md" };
                let big = if *ln > ENTRY_MAX_LINES { "  主入口超预算" } else { "" };
                crate::say_line!(
                    "  {}  {n:<26} {mark}  候选 {c}  引用 {rf}{big}",
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
            // references/ 一开始就建出来。
            //
            // 建目录这一步看着琐碎，实际决定习惯：目录不存在时，人（和模型）
            // 的默认动作就是把内容追加到 SKILL.md；目录摆在那里，"该拆"这件事
            // 就从"要额外做一步"变成"已经准备好了"。
            ensure_dir(&dir.join("references"))?;
            let st = stamps();
            let title = opt(a, "title");
            let title = if title.is_empty() { name.clone() } else { title };
            let skeleton = format!(
                "# {title}\n\n\
                 > 骨架由 `abook skill new` 生成。按 `abook skill` 的约定补全后再用。\n\
                 > 主入口预算 120 行；超了先拆到 references/ 再写，别把细节堆在这里。\n\n\
                 ## 何时用\n\n（触发条件：什么样的请求该用这份 Skill）\n\n\
                 ## 职责\n\n（这份 Skill 负责什么，不负责什么）\n\n\
                 ## 最短步骤\n\n1. \n2. \n\n\
                 ## 输出与完成条件\n\n（交付什么形态，怎样算做完）\n\n\
                 ## 参考\n\n\
                 细节、案例、参数表、排错、变体流程写进 references/ 下的专文，一份只管一个小方向。\n\
                 在这里逐行登记：文件名、管什么、什么情况下读。没登记的运行时发现不了，等于不存在。\n"
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
        "split" => {
            let name = match a.pos.get(1) {
                Some(n) => sanitize(n),
                None => return usage("需要 Skill 名，例如 `abook skill split my-skill --dry-run`"),
            };
            let dir = skill_dir(a, &name);
            let p = dir.join("SKILL.md");
            if !p.is_file() {
                return usage(format!("{} 不存在", p.display()));
            }
            let txt = read_str(&p)?;
            let budget: usize = opt(a, "max-entry").parse().unwrap_or(120);
            let keep: usize = opt(a, "keep").parse().unwrap_or(8);
            let plan = match plan_split(&txt, keep, budget) {
                Ok(x) => x,
                Err(e) => return usage(e),
            };
            if plan.sections.is_empty() {
                crate::say_line!("{name} 找不到可拆的 `##` 章节，什么也没做");
                return Ok(0);
            }
            if a.has("dry-run") {
                crate::say_line!("[dry-run] {name}: 主入口 {} 行 → 预计 {} 行（预算 {budget}）", plan.total, plan.entry_len);
                for s in &plan.sections {
                    crate::say_line!("  移出 {} 行 → references/{}", s.lines, s.slug);
                }
                return Ok(0);
            }
            if plan.entry_len > budget {
                crate::say_line!("注意：拆完主入口仍 {} 行，超过预算 {budget}。可以先调小 --keep，或手工再合章节。", plan.entry_len);
            }
            // 备份原文件。拆坏了要能原样退回来。
            let bak = dir.join(format!(".SKILL.md.bak-{}", crate::util::stamps().compact));
            write_atomic(&bak, &txt)?;
            ensure_dir(&dir.join("references"))?;
            for s in &plan.sections {
                let dst = dir.join("references").join(format!("{}.md", s.slug));
                if dst.is_file() {
                    // 不覆盖已有文件：那份可能是之前手工写好的，机器不该悄悄顶掉。
                    return blocked(format!(
                        "{} 已存在，不覆盖。先确认那份是不是要留着，再跑 split",
                        dst.display()
                    ));
                }
                write_atomic(&dst, &s.body)?;
            }
            write_atomic(&p, &plan.entry)?;
            // 拆完立刻自查：内容有没有丢、lint 过不过。
            let after = read_str(&p)?;
            let mut all = after.clone();
            for s in &plan.sections {
                all.push_str(&read_str(&dir.join("references").join(format!("{}.md", s.slug)))?);
            }
            let mut lost = Vec::new();
            for line in txt.lines() {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                if !all.contains(t) {
                    lost.push(t.to_string());
                }
            }
            crate::say_line!("{name} 已拆：主入口 {} 行 → {} 行，移出 {} 节", plan.total, after.lines().count(), plan.sections.len());
            for s in &plan.sections {
                crate::say_line!("  references/{}.md  {}", s.slug, s.lines);
            }
            crate::say_line!("备份 {}", bak.display());
            if lost.is_empty() {
                crate::say_line!("内容核对：无丢失");
            } else {
                crate::say_line!("内容核对：{} 行没找到，需要人工确认以下内容", lost.len());
                for l in lost.iter().take(8) {
                    crate::say_line!("  {}", crate::util::truncate_chars(l, 80));
                }
            }
            crate::say_line!("下一步：`abook skill lint {name}`");
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

    // ---- 主入口预算 ----
    //
    // 这一段以前根本没查。SKILL.md 被显式跳过（它是入口，不套"超 100 行须给
    // 目录"那条），于是入口写到 1032 行照样 `ok:true`。而入口恰恰是每次调用
    // 都要整篇进上下文的那个文件——它的体积是**每次**的成本，引用文件的体积
    // 只是**用到时**的成本，两者不该同等对待。
    //
    // 预算不是审美偏好：120 行足够写清触发、职责、最短步骤、输出与完成条件、
    // 导航。超出的部分按定义就是"当前任务用不到的细节"，它们属于引用文件。
    let budget = a.num("max-entry").unwrap_or(ENTRY_MAX_LINES as u64) as usize;
    let entry_lines = txt.lines().count();
    if entry_lines > budget {
        problems.push(format!(
            "主入口 {entry_lines} 行，超过预算 {budget} 行：先拆到 references/ 再写，别继续往 SKILL.md 里加{}",
            section_hint(&txt)
        ));
    }
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
                // 待决候选不是 Skill 的一部分——它还没被采纳。
                //
                // 候选里天然会贴路径、贴日志、贴报错原文，那是**证据**，不是
                // 运行时依赖。把它一起扫，结果是一份完全自包含的 Skill 只因为
                // 攒了几条候选就判不通过。未采纳的东西不该有否决权。
                if disp.starts_with("candidates/") || disp.starts_with("candidates\\") {
                    continue;
                }
                // 这个文件自己赋值过的变量名。
                let local_vars = assigned_vars(&sub);
                // 逐行套用与 SKILL.md 相同的判据
                for (n, line) in sub.lines().enumerate() {
                    let t = line.trim();
                    if has_abs_path(t) {
                        // 文档说"是什么"，脚本才是"要读什么"。
                        //
                        // SKILL.md 侧早就把绝对路径放在"需自行判断"档：写在文档里
                        // 解释系统布局（"`/sdcard/...` 通常不能写"）是正常的，只有
                        // 脚本真的去读某个本机路径才破坏可移植性。引用文件同样是文档，
                        // 却按硬问题报——同一类内容两套标准，结果是一份纯粹在讲
                        // Android 分区存储事实的表格永远挂四处红点。
                        //
                        // 分档落在文件类型上：`.md` 进 advisory，脚本与模板进 problems。
                        if disp.ends_with(".md") {
                            advisory.push(format!(
                                "{disp} 行{}: 出现了绝对路径（若为运行时要读的文件则不合规；仅作说明可忽略）: {}",
                                n + 1,
                                cut(t, 50)
                            ));
                        } else {
                            problems.push(format!(
                                "{disp} 行{}: 绝对路径（Skill 须自带依赖）: {}",
                                n + 1,
                                cut(t, 50)
                            ));
                        }
                    }
                    if has_relative_escape(t) {
                        problems.push(format!(
                            "{disp} 行{}: 相对路径逃出 Skill 目录: {}",
                            n + 1,
                            cut(t, 50)
                        ));
                    }
                    // 本文件自己赋过值的变量，不算"依赖本机环境"。
                    //
                    // `TMP="$(mktemp -d)"` 之后再用 `$TMP/...` 是脚本内部临时目录，
                    // 每台机器都能跑——mktemp 就是为此存在的。原来的判据看到 `$TMP/`
                    // 就报，因为 TMP 在 HOST_ENV_VARS 里。这会惩罚**所有**写临时目录
                    // 的验证脚本，而它们恰恰是最自包含的那一类。
                    if has_env_path_excluding(t, &local_vars) {
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
                // 规则是"超 100 行**须在头部给出它的目录**"，检查的应该是
                // "有没有目录"，而不是"是不是超了 100 行"。
                //
                // 原来只数行数就判不通过，从不看目录在不在：加完目录照样报。
                // 一个永远无法通过的检查，等价于把这条规则废掉——人会去
                // 绕过它，而真正没写目录的文件也被一起放过。
                // 只对**文档**要求目录。
                //
                // 代码文件（.py/.kt/.gradle/.sh）的自然导航是函数与类定义，
                // 在文件头上再挂一张目录是套错对象——143 行的 validate_skill.py
                // 里函数名就是它的目录，再让人手写一遍只是重复。硬套到代码上，
                // 结果是每个带脚本的 Skill 都永久挂一条修不掉的报警。
                let nlines = sub.lines().count();
                if disp.ends_with(".md") && nlines > 100 && !has_toc(&sub) {
                    problems.push(format!(
                        "{} 共 {nlines} 行但头部没有目录（超 100 行须在头部给目录）",
                        disp
                    ));
                }
            }
            // ---- 导航可达 ----
            //
            // "按需发现"的前提是入口能指到文件，两种失效方式各查一头：
            //   ① 文件在磁盘上，入口没登记 → 运行时没人知道它存在，等于不存在；
            //   ② 入口登记了，文件不在 → 照着读会扑空。
            //
            // 只认我自己这套目录约定（references/ scripts/ templates/ assets/
            // examples/ docs/），不去猜别的写法。猜错会把正确做法判成错误，那
            // 比漏检更糟——人会开始绕开这条检查，然后真问题也一起漏掉。
            for q in &files {
                let rel = q
                    .strip_prefix(&dir)
                    .unwrap_or(q)
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel == "SKILL.md" || rel == "decisions.log" || rel.starts_with("candidates/") {
                    continue;
                }
                if !is_ref_dir(&rel) {
                    continue;
                }
                let base = rel.rsplit('/').next().unwrap_or(&rel).to_string();
                if !txt.contains(&rel) && !txt.contains(&base) {
                    problems.push(format!(
                        "{rel} 没有在主入口登记：运行时发现不了它，等于不存在。请在导航里加一行（管什么 / 何时读）"
                    ));
                }
            }
            for cand in dangling_refs(&txt, &dir) {
                problems.push(format!(
                    "主入口登记了 `{cand}` 但文件不存在：导航指不到东西"
                ));
            }

            if a.has("json") {
                let arr = problems.iter().map(|p| jstr(p)).collect::<Vec<_>>().join(",");
                let adv = advisory.iter().map(|p| jstr(p)).collect::<Vec<_>>().join(",");
                crate::say_line!(
                    "{{\"skill\":{},\"problems\":[{}],\"advisory\":[{}],\"max_entry\":{budget},\"entry_lines\":{entry_lines},\"ok\":{}}}",
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
    // 名字紧邻 "skill"/"技能" 才算指代。
    //
    // 原来是"这一行只要同时出现 skill 和这个名字"就算引用。问题在于很多
    // Skill 名本身就是**普通英文词**：有一个叫 `design` 的 Skill，而
    // "This skill turns a design brief into a task list" 这句话里两个词都
    // 出现了，于是被判成"引用了另一个 Skill"。这句说的是 design brief
    // （设计简报，一份文件），跟那个 Skill 毫无关系。
    //
    // 判据收紧成"名字和 skill 紧紧挨着"，也就是真的在说"那个叫 X 的 skill"。
    let lower = line.to_lowercase();
    let n = name.to_lowercase();
    for sep in [" ", "-", "_"] {
        if lower.contains(&format!("{n}{sep}skill")) || lower.contains(&format!("skill{sep}{n}")) {
            return true;
        }
    }
    if line.contains(&format!("{name}技能")) || line.contains(&format!("技能{name}")) {
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
    // 标准设备不算"依赖本机文件"。
    //
    // `cat x 2>/dev/null` 里的 `/dev/null` 形态上确实是绝对路径，但它:
    //   - 每个 POSIX 系统都有，换台机器也在，不破坏可移植性；
    //   - 是 shell 重定向语法的一部分，不是 Skill 要读的什么东西。
    // 不放过它，凡是用过 `2>/dev/null` 压输出的 Skill 都会永远挂一条问题，
    // 而这条问题**没法修**——删掉重定向会让文档变差。一条永远修不掉的报警
    // 会训练人忽略整份报告，连带把真问题一起忽略。
    let std_dev = |i: usize| -> bool {
        for d in ["/dev/null", "/dev/stdout", "/dev/stderr", "/dev/stdin", "/dev/tty"] {
            let dc: Vec<char> = d.chars().collect();
            if i + dc.len() <= b.len() && b[i..i + dc.len()] == dc[..] {
                let after = if i + dc.len() < b.len() { b[i + dc.len()] } else { ' ' };
                if after.is_whitespace() || matches!(after, ';' | '|' | '&' | ')') {
                    return true;
                }
            }
        }
        false
    };
    for i in 0..b.len() {
        if b[i] == '/' {
            if in_url(i) || std_dev(i) {
                continue;
            }
            // 模式不是具体路径。
            //
            // Gradle 的 `excludes += ['/META-INF/{AL2.0,LGPL2.1}']`、Makefile 的
            // `$(BUILD)/*.o` 写的是"要排除/匹配什么形状"，不是本机上的某个文件。
            // 判成绝对路径等于禁止在构建配置里写排除规则，而那些规则跟换不换
            // 机器毫无关系，剔掉它们反而会打出更胖的包。
            if seg_has_pattern(&b, i) {
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

/// 头部有没有目录。
///
/// 三种写法都认，因为真实 Skill 里都出现过：
///   - 标题里带「目录」/「TOC」字样
///   - `L28  xxx` / `L28-40  xxx` 这种按行号定位的清单
///   - Markdown 锚点链接 `[xxx](#yyy)`
///
/// 只看前 40 行：目录是给"要不要往下读、从哪开始读"用的，塞在文件中间就
/// 失去意义了。
/// 主入口行数预算。
///
/// 120 行是"能写清触发、职责、最短步骤、输出与完成条件、导航"的宽松上限，
/// 不是美学数字：入口每次被调用都整篇进上下文，所以它的体积是**每次**成本。
/// 需要更多篇幅的内容，按定义就不是入口该装的东西。
const ENTRY_MAX_LINES: usize = 120;

/// Skill 内部允许存放引用文件的目录。只认这几个，其余一律不管。
///
/// 范围收紧是故意的：判据越宽，越容易把"文档里正常提到的路径"判成"未登记的
/// 引用文件"，而把正确答案判成错误答案会让人绕开整条检查。
const REF_DIRS: [&str; 6] = [
    "references/",
    "scripts/",
    "templates/",
    "assets/",
    "examples/",
    "docs/",
];

fn is_ref_dir(rel: &str) -> bool {
    REF_DIRS.iter().any(|d| rel.starts_with(d))
}

/// 主入口里最占行数的那几节，用于告诉人"该先拆哪一段"。
///
/// 只报行数没有可操作性——看到"超了 87 行"之后，下一步还是不知道动哪里。
fn section_hint(text: &str) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(h) = t.strip_prefix("## ").or_else(|| t.strip_prefix("# ")) {
            counts.push((h.trim().to_string(), 0));
        } else if let Some(last) = counts.last_mut() {
            last.1 += 1;
        }
    }
    if counts.is_empty() {
        return String::new();
    }
    counts.sort_by(|x, y| y.1.cmp(&x.1));
    let top: Vec<String> = counts
        .iter()
        .take(3)
        .filter(|(_, n)| *n > 0)
        .map(|(h, n)| format!("`{h}` {n}行"))
        .collect();
    if top.is_empty() {
        String::new()
    } else {
        format!("（最占行数：{}）", top.join("、"))
    }
}

/// 反引号包住的片段。导航是写给机器看的，约定用反引号标路径。
fn backticked(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut on = false;
    for c in text.chars() {
        if c == '`' {
            if on && !cur.is_empty() {
                out.push(cur.clone());
            }
            cur.clear();
            on = !on;
        } else if on {
            cur.push(c);
        }
    }
    out
}

/// 主入口登记了、磁盘上却没有的引用路径。
fn dangling_refs(txt: &str, dir: &std::path::Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in backticked(txt) {
        let t = tok.trim();
        if !is_ref_dir(t) || t.ends_with('/') {
            continue;
        }
        if t.contains("://") || t.starts_with('~') || t.starts_with('$') || t.starts_with('%') {
            continue;
        }
        // `references/{subcommand}.md`、`references/*.md` 这类是**写法示范**，
        // 不是登记了一个具体文件。把模式当路径判悬空，等于逼人别写模式——
        // 而模式恰恰是导航里最该有的东西。
        if t.contains(['{', '}', '<', '>', '*', '?', '[']) {
            continue;
        }
        // 这份路径到底指 Skill 自己的文件，还是用户项目里的文件？
        //
        // `references/foo.md` 出现在 Skill 里几乎一定是前者；而
        // `docs/brand-guidelines.md`、`assets/design-tokens.json` 这类在
        // Skill 正文里绝大多数是**叫用户去改他自己项目里的文件**——照"登记了
        // 文件不存在"判悬空，等于禁止 Skill 提到用户项目。
        //
        // 判据落在目录归属上：这段路径的首层目录**在本 Skill 里真实存在**，
        // 才当作 Skill 自己的文件来查。目录都不存在，它说的就是别人的目录。
        let seg = t.split('/').next().unwrap_or("");
        if seg.is_empty() || !dir.join(seg).is_dir() {
            continue;
        }
        if dir.join(t).exists() {
            continue;
        }
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// 这个文件里被赋值过的变量名（shell / makefile 的 `VAR=...` 与 `VAR = ...`）。
///
/// 用来区分"本脚本自己造的临时目录"和"依赖运行它的那台机器"。前者换机器没事，
/// 后者才会让 Skill 失效。
fn assigned_vars(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') || t.starts_with("//") {
            continue;
        }
        let bytes: Vec<char> = t.chars().collect();
        let mut i = 0;
        while i < bytes.len() {
            // 词首的大写标识符
            let word_start = i == 0
                || !(bytes[i - 1].is_alphanumeric() || bytes[i - 1] == '_');
            if word_start && bytes[i].is_ascii_uppercase() {
                let start = i;
                let mut j = i;
                while j < bytes.len()
                    && (bytes[j].is_ascii_uppercase() || bytes[j].is_ascii_digit() || bytes[j] == '_')
                {
                    j += 1;
                }
                let name: String = bytes[start..j].iter().collect();
                // 后面（允许隔空格）紧跟 `=` 但不是 `==`
                let mut k = j;
                while k < bytes.len() && bytes[k] == ' ' {
                    k += 1;
                }
                if k < bytes.len() && bytes[k] == '=' && bytes.get(k + 1) != Some(&'=') {
                    if !out.contains(&name) {
                        out.push(name);
                    }
                }
                i = j;
                continue;
            }
            i += 1;
        }
    }
    out
}

/// 与 `has_env_path` 相同，但放行本文件自己赋过值的变量。
fn has_env_path_excluding(t: &str, local: &[String]) -> bool {
    if local.is_empty() {
        return has_env_path(t);
    }
    // 把这些变量名从文本里"抹掉"后再判，比逐处解析 `$VAR` 简单且不改变判据。
    let mut masked = t.to_string();
    for v in local {
        masked = masked.replace(&format!("${{{v}}}"), "");
        masked = masked.replace(&format!("${v}"), "");
        masked = masked.replace(&format!("%{v}%"), "");
    }
    has_env_path(&masked)
}

/// 这段路径里是不是带着模式符（`{...}`、`*`、`?`、`[...]`、`$(...)`）。
///
/// 构建配置里的排除规则就是这样写的：它描述形状，不指向本机某个文件。
fn seg_has_pattern(b: &[char], slash_at: usize) -> bool {
    let mut j = slash_at;
    while j < b.len() {
        let c = b[j];
        if c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']' | '>' | '`' | ',') {
            break;
        }
        if matches!(c, '{' | '}' | '*' | '?' | '[') {
            return true;
        }
        // `$(VAR)` 是 make 变量
        if c == '$' && b.get(j + 1) == Some(&'(') {
            return true;
        }
        j += 1;
    }
    false
}

/// 一次章节拆分的计划。
struct SplitSection {
    /// references/ 下的文件名（不含扩展名）。
    slug: String,
    /// 这一节的原文（含标题）。
    body: String,
    /// 这一节有多少行。
    lines: usize,
    /// 通俗标题，写进导航表用。
    title: String,
}

struct SplitPlan {
    /// 拆分后的主入口全文。
    entry: String,
    /// 要移出去的每一节。
    sections: Vec<SplitSection>,
    /// 拆分前总行数。
    total: usize,
    /// 拆分后主入口预计行数。
    entry_len: usize,
    /// 用的是几级标题（2 = `##`，3 = `###`）。
    level: usize,
}

/// 把章节号转成文件名安全的 slug。
///
/// 中文标题不能直接当文件名（跨文件系统不可靠），所以优先用章节号 `01`、`02`，
/// 没有章节号就用 `section-N`。稳定、可预测、不会因为改了标题就重命名。
fn split_slug(no: &str, idx: usize, level: usize) -> String {
    let digits: String = no
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    let d = digits.trim_matches('-');
    if d.is_empty() {
        if level == 3 {
            format!("sub-{:02}", idx + 1)
        } else {
            format!("section-{:02}", idx + 1)
        }
    } else if level == 3 {
        format!("sub-{:0>2}", d)
    } else {
        format!("{:0>2}", d)
    }
}

/// 标题里的"名字"部分，去掉序号与装饰。
fn split_title(head: &str) -> String {
    let t = head.trim_start_matches('#').trim();
    let t = t.trim_start_matches(|c: char| {
        c.is_ascii_digit() || c == '.' || c == '、' || c == ')' || c == '）'
    });
    let t = t.trim_start_matches(|c: char| c == '.' || c == '、' || c == ':' || c == '：');
    let s = t.trim();
    if s.is_empty() {
        "（无标题）".to_string()
    } else {
        crate::util::truncate_chars(s, 40)
    }
}

/// 收集指定级别的标题行号（跳过代码围栏里的假标题）。
fn heading_lines(lines: &[&str], level: usize) -> Vec<usize> {
    let mark = format!("{} ", "#".repeat(level));
    let deeper = format!("{}#", mark);
    let mut out = Vec::new();
    let mut in_fence = false;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        if t.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // `## x` 命中 level=2；`### x` 不能算进 level=2。
        if t.starts_with(&mark) && !t.starts_with(&deeper) {
            out.push(i);
        }
    }
    out
}

/// 在指定级别上找切点：返回 (切点标题序号, 该节起点行)。
fn pick_cut(lines: &[&str], heads: &[usize], budget_eff: usize, keep_min: usize) -> (usize, usize) {
    let total = lines.len();
    let mut cut_idx = heads.len();
    // 从"留最多"往"留最少"退，第一个落进预算的就是答案。
    //
    // 下界是 keep_min，但它**只能让主入口更短、不能更宽**：预算才是硬要求，
    // keep_min 只是"别削得只剩一句看 references"的软下限。两个冲突时预算赢。
    // 原来把 keep_min 当硬下界，于是在 `rev-idapython`（41 个 `###`）上，
    // 留 8 节仍有几百行，检查永远不通过——把一个软偏好写成了不可越过的墙。
    let lower = {
        // 先看能不能满足 keep_min；满足不了就从 1 节开始找。
        let at_keep = heads.get(keep_min.max(1)).copied().unwrap_or(total);
        if lines[..at_keep].join("\n").lines().count() <= budget_eff {
            keep_min.max(1)
        } else {
            1
        }
    };
    for k in (lower..=heads.len()).rev() {
        let at = heads.get(k).copied().unwrap_or(total);
        if lines[..at].join("\n").lines().count() <= budget_eff {
            cut_idx = k;
            break;
        }
    }
    (cut_idx, heads.get(cut_idx).copied().unwrap_or(total))
}

/// 规划一次拆分：保留开头若干节，直到主入口回落到预算内，其余每节落到 references/。
///
/// 两个维度都要能动，缺一个就会在真实文件上卡住：
///
/// - **按行数决定切到哪**：预算是个行数，判断也该用行数。按节数会在长节文件上
///   失效（`rev-idapython` 只留 8 节仍剩 588 行，它前几节每节都上百行）。
/// - **`##` 不够就退到 `###`**：很多 Skill 只有一个 `## Common API`，里面 41 个
///   `###`。只在 `##` 上找切点，这类文件永远"没什么可拆的"，而它们正是最该拆的。
///
/// 先用 `##`，回落到预算内就用它；否则用 `###`。两者都不行就报错让用户手工分。
fn plan_split(txt: &str, keep_min: usize, budget: usize) -> Result<SplitPlan, String> {
    let lines: Vec<&str> = txt.split('\n').collect();
    let total = lines.len();
    if total <= budget {
        return Err(format!("{total} 行，已经在预算 {budget} 以内，不用拆"));
    }
    let mut last_err = String::from("找不到可拆的标题");
    // 一条标题都没有的扁平文档：按段落块顺序切。
    //
    // 提示词库、对照表这类文件常常整篇没有 `##`——它本来就是"一次读完"的东西。
    // 遇到这种，按标题找切点永远失败，但它确实超预算，也确实能拆：按空行分段，
    // 顺序拼进若干份 references，导航表注明"按顺序读"。顺序是它的语义，拆了
    // 顺序不乱，内容一行不少。
    let flat_ok = heading_lines(&lines, 2).len() < 2 && heading_lines(&lines, 3).len() < 2;
    if flat_ok {
        if let Some(plan) = plan_split_flat(txt, budget) {
            return Ok(plan);
        }
    }
    for level in [2usize, 3usize] {
        let heads = heading_lines(&lines, level);
        if heads.len() < 2 {
            last_err = format!(
                "`{}` 级标题只有 {} 个，拆不动",
                "#".repeat(level),
                heads.len()
            );
            continue;
        }
        // 导航表要占几行：表头 + 每节一行 + 空行。
        let nav_overhead = 6 + heads.len();
        let budget_eff = budget.saturating_sub(nav_overhead);
        let (cut_idx, split_at) = pick_cut(&lines, &heads, budget_eff, keep_min);
        if cut_idx >= heads.len() {
            last_err = format!(
                "`{}` 级标题留 {} 节仍超预算",
                "#".repeat(level),
                keep_min.max(1)
            );
            continue;
        }
        let head_part = lines[..split_at].join("\n");
        let mut sections = Vec::new();
        for (k, &h) in heads.iter().enumerate().skip(cut_idx) {
            let end = heads.get(k + 1).copied().unwrap_or(total);
            let body = lines[h..end].join("\n");
            let raw_head = lines[h].trim();
            let no: String = raw_head
                .trim_start_matches('#')
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            sections.push(SplitSection {
                slug: split_slug(&no, k - cut_idx, level),
                title: split_title(raw_head),
                lines: body.lines().count(),
                body: body.trim_end().to_string() + "\n",
            });
        }
        if sections.is_empty() {
            last_err = "切点落在末尾，没有可移出的章节".to_string();
            continue;
        }
        let mut nav = String::new();
        nav.push_str("## 参考\n\n");
        nav.push_str("细节按需读，不要一次全加载。主入口只留骨架与最短路径，展开内容在下列文件里。\n\n");
        nav.push_str("| 文件 | 管什么 |\n|---|---|\n");
        for s in &sections {
            nav.push_str(&format!("| `references/{}.md` | {} |\n", s.slug, s.title));
        }
        let entry = format!("{}\n\n{}\n", head_part.trim_end(), nav);
        let entry_len = entry.lines().count();
        if entry_len > budget {
            last_err = format!("`{}` 级标题拆完仍 {entry_len} 行", "#".repeat(level));
            continue;
        }
        return Ok(SplitPlan {
            entry,
            sections,
            total,
            entry_len,
            level,
        });
    }
    Err(last_err)
}

/// 无标题文档的顺序切分。
///
/// 按空行分段，逐段累加，装满一份就开下一份。顺序保持原样，导航表注明按序读。
fn plan_split_flat(txt: &str, budget: usize) -> Option<SplitPlan> {
    let lines: Vec<&str> = txt.split('\n').collect();
    let total = lines.len();
    if total <= budget {
        return None;
    }
    // 标题前的头部（frontmatter、开场说明）留在主入口。
    //
    // 一份 `---` frontmatter 加几句开场，通常是它唯一的"门面"，拆走会让主入口
    // 变成空壳。留到第一个空行块结束为止，上限取预算的 1/3。
    let head_limit = (budget / 3).max(10);
    let mut head_end = 0;
    for (i, l) in lines.iter().enumerate() {
        if i >= head_limit {
            break;
        }
        if l.trim().is_empty() && i > 3 {
            head_end = i + 1;
            break;
        }
    }
    let head_part = lines[..head_end].join("\n");
    let body: Vec<&str> = lines[head_end..].to_vec();
    if body.iter().all(|l| l.trim().is_empty()) {
        return None;
    }
    // 按空行分段
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for l in &body {
        if l.trim().is_empty() {
            if !cur.is_empty() {
                blocks.push(cur);
                cur = Vec::new();
            }
        } else {
            cur.push(l);
        }
    }
    if !cur.is_empty() {
        blocks.push(cur);
    }
    if blocks.len() < 2 {
        return None;
    }
    // 每份目标大小：让份数不至于太多，又不超预算。
    let target = ((total as f64) / 4.0).max(40.0) as usize;
    let mut chunks: Vec<String> = Vec::new();
    let mut acc: Vec<String> = Vec::new();
    let mut acc_lines = 0;
    for b in &blocks {
        let bl = b.len();
        if acc_lines > 0 && acc_lines + bl > target {
            chunks.push(acc.join("\n\n"));
            acc.clear();
            acc_lines = 0;
        }
        acc.push(b.join("\n"));
        acc_lines += bl + 1;
    }
    if !acc.is_empty() {
        chunks.push(acc.join("\n\n"));
    }
    if chunks.len() < 2 {
        return None;
    }
    let mut sections = Vec::new();
    for (i, c) in chunks.iter().enumerate() {
        let body = format!(
            "# 第 {} 部分（全文第 {} 段续）\n\n> 本文件是顺序阅读的一部分，需与前后各份连起来看。\n\n{}\n",
            i + 1,
            i + 1,
            c
        );
        sections.push(SplitSection {
            slug: format!("part-{:02}", i + 1),
            title: format!("第 {} 部分（按顺序读）", i + 1),
            lines: body.lines().count(),
            body,
        });
    }
    let mut nav = String::new();
    nav.push_str("## 参考\n\n");
    nav.push_str("本文件无分节标题，全文按顺序切成若干份。**必须按编号顺序读**，顺序即语义。\n\n");
    nav.push_str("| 文件 | 管什么 |\n|---|---|\n");
    for s in &sections {
        nav.push_str(&format!("| `references/{}.md` | {} |\n", s.slug, s.title));
    }
    let entry = format!("{}\n\n{}\n", head_part.trim_end(), nav);
    let entry_len = entry.lines().count();
    if entry_len > budget {
        return None;
    }
    Some(SplitPlan {
        entry,
        sections,
        total,
        entry_len,
        level: 0,
    })
}

fn has_toc(text: &str) -> bool {
    let mut hits = 0;
    for line in text.lines().take(40) {
        let t = line.trim();
        if t.contains("目录") || t.contains("TOC") || t.contains("Table of Contents") {
            return true;
        }
        // `L28  xxx`、`L28-40 xxx`、`行28 xxx`
        let b: Vec<char> = t.chars().collect();
        let mut i = 0;
        while i < b.len() {
            let mark = b[i] == 'L' || (i > 0 && b[i] == '#' && b[i - 1] == 'L');
            if mark {
                let mut j = i + 1;
                while j < b.len() && (b[j].is_ascii_digit() || b[j] == '-' || b[j] == ',') {
                    j += 1;
                }
                if j > i + 1 && j < b.len() && b[j].is_whitespace() {
                    hits += 1;
                    break;
                }
            }
            i += 1;
        }
        if t.contains("](#") {
            hits += 1;
        }
    }
    hits >= 3
}
