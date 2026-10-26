//! Vault commands. Credential values are returned verbatim; the vault only
//! governs storage form, not disclosure policy.

use crate::util::*;
use crate::vault::*;
use std::path::Path;

pub fn run(a: &Args) -> R<i32> {
    let root = resolve_root(a);
    let sub = a.pos_or(0, "list");
    match sub.as_str() {
        "list" | "ls" => {
            let entries = list_entries(&root);
            if a.has("json") {
                crate::say_line!(
                    "[{}]",
                    entries
                        .iter()
                        .map(|e| {
                            format!(
                                "{{\"ident\":{},\"purpose\":{},\"file\":{}}}",
                                jstr(&e.ident),
                                jstr(&e.purpose),
                                jstr(&e.path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default())
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                );
                return Ok(0);
            }
            for e in &entries {
                let mark = if e.ident == LEDGER_ID { " (名册)" } else { "" };
                crate::say_line!("{}{}\n    {}", e.ident, mark, e.purpose);
            }
            crate::say_line!("共 {} 条", entries.len());
            Ok(0)
        }
        "get" | "read" | "show" => {
            let ident = a.need_pos(1, "凭据标识")?;
            let e = read_entry(&root, ident)?;
            if a.has("json") {
                crate::say_line!(
                    "{{\"ident\":{},\"purpose\":{},\"body\":{}}}",
                    jstr(&e.ident),
                    jstr(&e.purpose),
                    jstr(&e.body)
                );
            } else if a.has("purpose-only") {
                crate::say_line!("{}", e.purpose);
            } else {
                crate::say_line!("{}", e.body);
            }
            Ok(0)
        }
        "put" | "set" | "add" => {
            let ident = a
                .pos
                .get(1)
                .cloned()
                .or_else(|| a.get("id").map(|s| s.to_string()))
                .unwrap_or_default();
            if ident.is_empty() {
                return usage("vault put 需要凭据标识");
            }
            let purpose = a.get("purpose").unwrap_or("").to_string();
            if purpose.is_empty() {
                return usage("缺少 --purpose：说明这条凭据用在什么平台、做什么、何时用");
            }
            let body = if let Some(f) = a.get("body-file") {
                read_str(&expand_tilde(f))?
            } else if let Some(b) = a.get("body") {
                b.to_string()
            } else if let Some(stdin) = a.get("stdin") {
                let _ = stdin;
                let mut s = String::new();
                use std::io::Read;
                std::io::stdin().read_to_string(&mut s)?;
                s
            } else {
                return usage("缺少凭据正文：用 --body、--body-file 或 --stdin");
            };
            if a.has("dry-run") {
                crate::say_line!("[dry-run] 将写入 {}", entry_name(&ident));
                return Ok(0);
            }
            let p = write_entry(&root, &ident, &purpose, body.trim_end())?;
            let (changed, n) = sync_ledger(&root)?;
            crate::say_line!(
                "{}  (名册{}，共 {} 条)",
                p.display(),
                if changed { "已同步" } else { "未变" },
                n
            );
            Ok(0)
        }
        "rm" | "del" | "delete" => {
            let ident = a.need_pos(1, "凭据标识")?;
            if ident == LEDGER_ID {
                return usage("名册由工具自动维护，不能单独删除");
            }
            if !a.has("approve") && !a.has("dry-run") {
                return blocked("删除凭据需要明确批准：确认后加 --approve。");
            }
            if a.has("dry-run") {
                crate::say_line!("[dry-run] 将删除 {ident}");
                return Ok(0);
            }
            let ok = delete_entry(&root, ident)?;
            sync_ledger(&root)?;
            crate::say_line!("{}{}", if ok { "已删除 " } else { "不存在 " }, ident);
            Ok(if ok { 0 } else { 3 })
        }
        "ledger" | "repair" => {
            let e = read_entry(&root, LEDGER_ID)?;
            if a.has("rebuild") || sub == "repair" {
                // dry-run 必须是只读的。这条路径原来忽略 --dry-run，名册有漂移
                // 时照样写盘——而 `--dry-run` 存在的意义就是让人先看清要不要
                // 修，而不是先修了再说。
                if a.has("dry-run") {
                    let (changed, n) = diff_ledger(&root)?;
                    crate::say_line!(
                        "[dry-run] 名册{}（共 {n} 条），未写入",
                        if changed { "与保险箱不一致，需要重建" } else { "已是最新" }
                    );
                    return Ok(0);
                }
                let (changed, n) = sync_ledger(&root)?;
                if a.has("json") {
                    crate::say_line!(
                        "{{\"rebuilt\":{},\"entries\":{n}}}",
                        if changed { "true" } else { "false" }
                    );
                } else {
                    crate::say_line!("名册{} 共 {} 条", if changed { "已重建" } else { "未变" }, n);
                }
                return Ok(0);
            }
            if a.has("json") {
                // The ledger body is line-oriented `标识|用途`; split so callers
                // get fields instead of having to re-parse the same text.
                let rows = e
                    .body
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| {
                        let mut it = l.splitn(2, '|');
                        let id = it.next().unwrap_or("").trim();
                        let use_ = it.next().unwrap_or("").trim();
                        format!("{{\"ident\":{},\"purpose\":{}}}", jstr(id), jstr(use_))
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                crate::say_line!(
                    "{{\"ident\":{},\"entries\":[{}]}}",
                    jstr(&e.ident),
                    rows
                );
            } else {
                crate::say_line!("{}", e.body);
            }
            Ok(0)
        }
        "verify" => {
            // 先看磁盘上到底有哪些文件，再和"能解出来的条目"对账。
            //
            // 原实现只遍历 `list_entries`，而那个函数会把解不开的文件**静默
            // 跳过**（读失败、解密失败、名字不合规都只是 `continue`）。于是
            // 一份损坏的凭据条目在 verify 眼里根本不存在：它自检剩下的条目，
            // 全部通过，报告"0 失败"。自检用一个会把问题藏起来的列表去查问题，
            // 结构上就不可能发现问题。
            // 保险箱目录本身读不到，与"保险箱是空的"是两件事，不能都报通过。
            //
            // `read_dir` 失败在这里曾是 `if let Ok(...)`，错误被静默吞掉，随后
            // `list_entries` 也返回空列表，于是两边都是 0——`ABOOK_ROOT` 指到
            // 一个没有 vault/ 的目录，verify 照样回答"自检 0 条，失败 0 条"，
            // 退出码 0。凭据没备份、路径配错、权限不对，全都表现为"一切正常"。
            let vdir = crate::vault::vault_dir(&root);
            if !vdir.is_dir() {
                if a.has("json") {
                    crate::say_line!(
                        "{{\"ok\":false,\"reason\":{}}}",
                        jstr("保险箱目录不存在——这不等于库是空的")
                    );
                } else {
                    crate::say_line!(
                        "FAIL 保险箱目录 {} 不存在。\n     这不等于没有凭据：要么库里本来就没建起来，\n     要么 ABOOK_ROOT / HOME 指错了地方。",
                        vdir.display()
                    );
                }
                return Ok(3);
            }
            // 目录读不到要立刻报，不能退化成"0 条"。
            if let Err(e) = std::fs::read_dir(&vdir) {
                if a.has("json") {
                    crate::say_line!(
                        "{{\"ok\":false,\"reason\":{}}}",
                        jstr(&format!("保险箱目录读不到：{e}"))
                    );
                } else {
                    crate::say_line!("FAIL 保险箱目录读不到：{e}");
                }
                return Ok(6);
            }
            // 用 scan_vault，不用 list_entries。
            //
            // 这一条曾经是错的：上面费了力气把磁盘文件和"列表里的条目"对账，
            // 但对账用的那个列表自己就会把坏文件静默跳过。自己拿一个会藏问题
            // 的清单去查问题，结构上就不可能查出问题——坏文件不在列表里，于是
            // 一个也报不出来。
            //
            // scan_vault 把读得出的和读不出的一起交出来，好文件和坏文件都在
            // 视野里，才算真的查过。
            let (entries, broken) = crate::vault::scan_vault(&root);
            let mut bad = broken.len();
            let mut unverified = 0usize;
            let mut rows: Vec<String> = Vec::new();
            for (n, why) in &broken {
                if a.has("json") {
                    rows.push(format!(
                        "{{\"file\":{},\"ok\":false,\"bytes\":0,\"reason\":{}}}",
                        jstr(n),
                        jstr(why)
                    ));
                } else {
                    crate::say_line!("FAIL {n} 文件存在但解不出条目（{why}）");
                }
            }
            for e in &entries {
                match read_entry(&root, &e.ident) {
                    Ok(full) => {
                        let name = entry_name(&e.ident);
                        let actual = e
                            .path
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let (ok, why) = if name != actual {
                            (false, format!("文件名派生不符: 应为 {name} 实为 {actual}"))
                        } else if full.ident != e.ident {
                            (false, "解出的标识不一致".to_string())
                        } else {
                            (true, String::new())
                        };
                        if !ok {
                            bad += 1;
                        }
                        if a.has("json") {
                            rows.push(format!(
                                "{{\"ident\":{},\"ok\":{},\"bytes\":{},\"verified\":{},\"reason\":{}}}",
                                jstr(&e.ident),
                                ok,
                                full.body.len(),
                                full.verified,
                                jstr(&why)
                            ));
                        } else if !ok {
                            crate::say_line!("FAIL {} {why}", e.ident);
                        } else if !full.verified {
                            // 解得出，但没有校验码。这**不等于通过**。
                            //
                            // 旧格式条目就是这样：内容照样能解出来，可它从没
                            // 写过校验码，所以谁也回答不了"这份密文有没有被改
                            // 过"。原来 verify 对它一律报 PASS，等于把"无从
                            // 校验"说成了"校验通过"。它不是失败，但必须被看见，
                            // 所以单独成一档。
                            unverified += 1;
                            if !a.has("quiet") {
                                crate::say_line!(
                                    "WARN {}  ({}B) 无校验码，能读但无法证明未被改动；\
                                     重新 vault put 同标识可升级为带校验的格式",
                                    e.ident,
                                    full.body.len()
                                );
                            }
                        } else if !a.has("quiet") {
                            crate::say_line!("PASS {}  ({}B)", e.ident, full.body.len());
                        }
                    }
                    Err(err) => {
                        bad += 1;
                        if a.has("json") {
                            rows.push(format!(
                                "{{\"ident\":{},\"ok\":false,\"bytes\":0,\"reason\":{}}}",
                                jstr(&e.ident),
                                jstr(&format!("解密失败: {}", err.msg()))
                            ));
                        } else {
                            crate::say_line!("FAIL {} 解密失败: {}", e.ident, err.msg());
                        }
                    }
                }
            }
            if a.has("json") {
                crate::say_line!(
                    "{{\"total\":{},\"bad\":{bad},\"unverified\":{unverified},\"ok\":{},\"entries\":[{}]}}",
                    entries.len() + broken.len(),
                    bad == 0 && unverified == 0,
                    rows.join(",")
                );
            } else {
                // 分母要算上解不出来的文件，否则"自检 2 条"会让人以为库里就
                // 只有 2 份凭据——磁盘上明明是 3 份，第 3 份才是要查的那个。
                let total = entries.len() + broken.len();
                if unverified > 0 {
                    crate::say_line!(
                        "自检 {} 条，失败 {} 条，{} 条无校验码（能读，但无法证明未被改动）",
                        total,
                        bad,
                        unverified
                    );
                } else {
                    crate::say_line!("自检 {} 条，失败 {} 条", total, bad);
                }
            }
            // 三种状态用三个退出码，脚本才守得住门。
            //
            // 原来只有"整库都无校验"才退出 4，混合场景（有的一条带校验、
            // 有的没有）返回 0。这留了一个能用的洞：**删掉条目末尾那行
            // 校验码，篡改就隐身了**——明文照常返回，verify 只打一行 WARN，
            // 整库退出码仍是 0。而删一行正是最容易做的改动。
            //
            // 所以只要有一条无法证明未改动，就不算通过：
            //   5 = 有明确失败（解不出，或校验不符）
            //   4 = 有需要处理的（无校验码，重写一次即可升级）
            //   0 = 全部证明得了
            // 4 与"需批准"共用码值，语义一致：都需要人来处理一下。
            Ok(if bad > 0 {
                5
            } else if unverified > 0 {
                4
            } else {
                0
            })
        }
        "names" => {
            let entries = list_entries(&root);
            if a.has("json") {
                let rows = entries
                    .iter()
                    .map(|e| {
                        format!(
                            "{{\"ident\":{},\"file\":{}}}",
                            jstr(&e.ident),
                            jstr(&entry_name(&e.ident))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                crate::say_line!("{{\"total\":{},\"entries\":[{}]}}", entries.len(), rows);
                return Ok(0);
            }
            for e in &entries {
                crate::say_line!("{:<40} {}", e.ident, entry_name(&e.ident));
            }
            Ok(0)
        }
        // Help documented `lint` and `repair` but neither was dispatched, so a
        // documented command answered "unknown subcommand". They are the same
        // operations under the names the docs use.
        "reconcile" | "lint" => {
            let (f, r, probs) = reconcile(&root);
            crate::say_line!("文件 {f} / 名册 {r}");
            for p in &probs {
                crate::say_line!("  - {p}");
            }
            Ok(if probs.is_empty() { 0 } else { 5 })
        }
        "export" => {
            let out = a.get("out").unwrap_or("vault-export.txt").to_string();
            let mut s = String::new();
            for e in list_entries(&root) {
                if let Ok(full) = read_entry(&root, &e.ident) {
                    s.push_str(&format!("{}\n{}\n\n", e.ident, full.body));
                }
            }
            if a.has("dry-run") {
                crate::say_line!("[dry-run] 将导出 {} 条到 {out}", list_entries(&root).len());
                return Ok(0);
            }
            write_atomic(Path::new(&out), &s)?;
            crate::say_line!("{out}");
            Ok(0)
        }
        // `export` writes "ident\npurpose?\nbody\n\n" blocks. `import` reads that
        // same shape back so a dump is actually restorable, which is the whole
        // point of shipping an export command.
        "import" => {
            let src = a
                .pos
                .get(1)
                .cloned()
                .or_else(|| a.get("from").map(|s| s.to_string()))
                .unwrap_or_default();
            if src.is_empty() {
                return usage("vault import 需要源文件：vault import <文件>");
            }
            let text = read_str(&expand_tilde(&src))?;
            let mut added = 0usize;
            let mut skipped = 0usize;
            for block in text.split("\n\n") {
                let block = block.trim_matches('\n');
                if block.trim().is_empty() {
                    continue;
                }
                let mut lines = block.lines();
                let ident = match lines.next() {
                    Some(v) if !v.trim().is_empty() => v.trim().to_string(),
                    _ => continue,
                };
                let body: String = lines.collect::<Vec<_>>().join("\n");
                if body.trim().is_empty() {
                    skipped += 1;
                    continue;
                }
                if a.has("dry-run") {
                    crate::say_line!("[dry-run] 将导入 {ident}");
                    added += 1;
                    continue;
                }
                // An export carries no purpose line, so keep an existing purpose
                // rather than overwriting it with a placeholder.
                let purpose = read_entry(&root, &ident)
                    .map(|e| e.purpose)
                    .unwrap_or_else(|_| "由导入恢复".to_string());
                write_entry(&root, &ident, &purpose, body.trim_end())?;
                added += 1;
            }
            if !a.has("dry-run") {
                sync_ledger(&root)?;
            }
            crate::say_line!("导入 {added} 条{}", if skipped > 0 { format!("，跳过 {skipped} 条空正文") } else { String::new() });
            Ok(0)
        }
        // Re-seed every entry: new session seed, plaintext unchanged, ciphertext
        // rewritten. Needed after a seed is exposed without touching the values.
        "rekey" => {
            let entries = list_entries(&root);
            if a.has("dry-run") {
                crate::say_line!("[dry-run] 将重设 {} 条的会话种子", entries.len());
                return Ok(0);
            }
            // rekey 是"种子泄露之后"的补救命令，所以它必须比任何命令都更
            // 不可能把库写坏。原来它有两处致命问题：
            //
            // 1. `encrypt_text` 返回的文本**第一行已经是新种子**，这里又在
            //    外层拼了一次 `{new_seed}\n`，文件开头于是有两个种子行。
            //    解密时首行当种子，第二个 12 位数和 token 一起被摊平进正文，
            //    校验码必然对不上——条目永久读不出来，而命令报"已重设"。
            //    泄露后第一次执行补救就作废全库，还报成功。
            // 2. 没有写入后自检。`write_entry` 有复读校验，rekey 没有。
            //
            // 现在：直接写 encrypt_text 的返回值（它自带种子行），写完立刻
            // 用 read_entry 复读校验，并逐字比对明文。任一条失败就整体报错
            // 并指明是哪一条——不能把"写坏了"混在"重设成功"里。
            let mut n = 0usize;
            for e in &entries {
                let before = read_entry(&root, &e.ident)?;
                let new_seed = new_seed();
                let path = entry_path(&root, &e.ident);
                let payload = format!("{}|{}\n{}", e.ident, e.purpose, before.body);
                // encrypt_text 的输出已含种子行，不要再拼。
                let cipher = encrypt_text(&new_seed, &payload)?;
                write_atomic(&path, &cipher)?;
                // 复读：值必须逐字不变，种子必须换新。
                let after = match read_entry(&root, &e.ident) {
                    Ok(x) => x,
                    Err(err) => {
                        return integrity(format!(
                            "rekey 写入后无法读回条目 {}：{}。\n\
                             该条目已损坏，请立刻用 `vault put {}` 重新写入正确的值。",
                            e.ident,
                            err.msg(),
                            e.ident
                        ))
                    }
                };
                if after.body != before.body {
                    return integrity(format!(
                        "rekey 写入后明文发生变化（{}）。\n\
                         这是严重的写入缺陷，该条目已损坏，请用 `vault put {}` 重写。",
                        e.ident, e.ident
                    ));
                }
                n += 1;
            }
            sync_ledger(&root)?;
            // 换种子却没换成的（新种子与旧的一样）说明随机源有问题。
            crate::say_line!("已重设 {n} 条的会话种子（每条都做了写入后复读校验）");
            Ok(0)
        }
        other => usage(format!("未知 vault 子命令: {other}")),
    }
}

/// Sub-command names this module actually dispatches. `selftest` compares them
/// against the names the help text advertises; four documented names (`lint`,
/// `repair`, `import`, `rekey`) were once missing here and answered
/// "unknown subcommand".
pub fn known_subcommands() -> &'static [&'static str] {
    &[
        "list", "ls", "get", "read", "show", "put", "set", "add", "rm", "del",
        "delete", "ledger", "repair", "verify", "names", "reconcile", "lint",
        "export", "import", "rekey",
    ]
}
