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
    home_dir().join("AgentSkills")
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
            let mut rows: Vec<(String, usize, usize)> = Vec::new();
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
                    rows.push((name, have, count_md(&p.join("candidates"))));
                }
            }
            rows.sort();
            if a.has("json") {
                let arr = rows
                    .iter()
                    .map(|(n, h, c)| {
                        format!(
                            "{{\"name\":{},\"has_skill_md\":{},\"candidates\":{}}}",
                            jstr(n),
                            h == &1,
                            c
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                crate::say_line!("{{\"root\":{},\"skills\":[{}]}}", jstr(&root.display().to_string()), arr);
                return Ok(0);
            }
            crate::say_line!("{}  共 {} 个", root.display(), rows.len());
            for (n, h, c) in &rows {
                let mark = if *h == 1 { "有SKILL.md" } else { "缺SKILL.md" };
                crate::say_line!("  {n:<28} {mark}  候选 {c}");
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
            ensure_dir(&cdir)?;
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
            let mut problems: Vec<String> = Vec::new();
            for (n, line) in txt.lines().enumerate() {
                let ln = n + 1;
                let t = line.trim();
                // 绝对路径：原来只认 `/home/` 和 `C:\`，`/tmp/`、`/root/`、
                // 盘符任意位置都能漏过去。判据应该是"任何以 / 或 X:\ 开头的
                // 路径片段"，而不是列举几个我想到过的前缀。
                if has_abs_path(t) {
                    problems.push(format!("行{ln}: 绝对路径（Skill 须自带依赖）: {}", cut(t, 60)));
                }
                // 相对逃逸：`../` 会读到 Skill 目录之外，同样破坏自包含。
                if t.contains("../") {
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
            }
            // 跨 Skill 引用：拿同级目录里真实存在的 Skill 名去比对，而不是只认
            // 几个我写死的短语。引用一个确实存在的兄弟 Skill 是最常见的越界方式。
            if let Some(parent) = dir.parent() {
                if let Ok(rd) = fs::read_dir(parent) {
                    let siblings: Vec<String> = rd
                        .flatten()
                        .filter(|e| e.path().is_dir())
                        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                        .filter(|s| s != &name && !s.starts_with('.'))
                        .collect();
                    for (n, line) in txt.lines().enumerate() {
                        let t = line.trim();
                        for sib in &siblings {
                            if t.contains(sib.as_str()) {
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
                    if t.contains("../") {
                        problems.push(format!(
                            "{disp} 行{}: 相对路径逃出 Skill 目录: {}",
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
                                if sib != name && !sib.starts_with('.') && t.contains(sib.as_str()) {
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
                let arr = problems
                    .iter()
                    .map(|p| jstr(p))
                    .collect::<Vec<_>>()
                    .join(",");
                crate::say_line!(
                    "{{\"skill\":{},\"problems\":[{}],\"ok\":{}}}",
                    jstr(&name),
                    arr,
                    problems.is_empty()
                );
                return Ok(0);
            }
            if problems.is_empty() {
                crate::say_line!("{} 通过：无外部路径、无跨 Skill 引用、无占位词", name);
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
        if nm.starts_with('.') || nm == "node_modules" || nm == "target" {
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

fn has_abs_path(t: &str) -> bool {
    let b: Vec<char> = t.chars().collect();
    for i in 0..b.len() {
        if b[i] == '/' && i > 0 {
            let prev = b[i - 1];
            let boundary = prev.is_whitespace()
                || matches!(prev, '"' | '\'' | '(' | '[' | '=' | ':' | '`' | '>');
            let nxt = b.get(i + 1).copied().unwrap_or(' ');
            if boundary && nxt.is_alphabetic() {
                return true;
            }
        }
        // 盘符：字母 + `:` + `\` 或 `/`
        if b[i].is_ascii_alphabetic()
            && b.get(i + 1) == Some(&':')
            && matches!(b.get(i + 2), Some('\\') | Some('/'))
        {
            return true;
        }
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
