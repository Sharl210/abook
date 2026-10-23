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
            let entries = list_entries(&root);
            let mut bad = 0;
            let mut rows: Vec<String> = Vec::new();
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
                                "{{\"ident\":{},\"ok\":{},\"bytes\":{},\"reason\":{}}}",
                                jstr(&e.ident),
                                ok,
                                full.body.len(),
                                jstr(&why)
                            ));
                        } else if !ok {
                            crate::say_line!("FAIL {} {why}", e.ident);
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
                    "{{\"total\":{},\"bad\":{bad},\"ok\":{},\"entries\":[{}]}}",
                    entries.len(),
                    bad == 0,
                    rows.join(",")
                );
            } else {
                crate::say_line!("自检 {} 条，失败 {} 条", entries.len(), bad);
            }
            Ok(if bad > 0 { 5 } else { 0 })
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
            let mut n = 0usize;
            for e in &entries {
                let full = read_entry(&root, &e.ident)?;
                let new_seed = new_seed();
                let path = entry_path(&root, &e.ident);
                let payload = format!("{}|{}\n{}", e.ident, e.purpose, full.body);
                let cipher = encrypt_text(&new_seed, &payload)?;
                write_atomic(&path, &format!("{new_seed}\n{cipher}\n"))?;
                n += 1;
            }
            sync_ledger(&root)?;
            crate::say_line!("已重设 {n} 条的会话种子");
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
