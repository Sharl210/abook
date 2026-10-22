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
                println!(
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
                println!("{}{}\n    {}", e.ident, mark, e.purpose);
            }
            println!("共 {} 条", entries.len());
            Ok(0)
        }
        "get" | "read" | "show" => {
            let ident = a.need_pos(1, "凭据标识")?;
            let e = read_entry(&root, ident)?;
            if a.has("json") {
                println!(
                    "{{\"ident\":{},\"purpose\":{},\"body\":{}}}",
                    jstr(&e.ident),
                    jstr(&e.purpose),
                    jstr(&e.body)
                );
            } else if a.has("purpose-only") {
                println!("{}", e.purpose);
            } else {
                println!("{}", e.body);
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
                println!("[dry-run] 将写入 {}", entry_name(&ident));
                return Ok(0);
            }
            let p = write_entry(&root, &ident, &purpose, body.trim_end())?;
            let (changed, n) = sync_ledger(&root)?;
            println!(
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
                println!("[dry-run] 将删除 {ident}");
                return Ok(0);
            }
            let ok = delete_entry(&root, ident)?;
            sync_ledger(&root)?;
            println!("{}{}", if ok { "已删除 " } else { "不存在 " }, ident);
            Ok(if ok { 0 } else { 3 })
        }
        "ledger" => {
            let e = read_entry(&root, LEDGER_ID)?;
            if a.has("rebuild") {
                let (changed, n) = sync_ledger(&root)?;
                println!("名册{} 共 {} 条", if changed { "已重建" } else { "未变" }, n);
                return Ok(0);
            }
            println!("{}", e.body);
            Ok(0)
        }
        "verify" => {
            let entries = list_entries(&root);
            let mut bad = 0;
            for e in &entries {
                match read_entry(&root, &e.ident) {
                    Ok(full) => {
                        let name = entry_name(&e.ident);
                        let actual = e
                            .path
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        if name != actual {
                            println!("FAIL {} 文件名派生不符: 应为 {name} 实为 {actual}", e.ident);
                            bad += 1;
                        } else if full.ident != e.ident {
                            println!("FAIL {} 解出的标识不一致: {}", e.ident, full.ident);
                            bad += 1;
                        } else if !a.has("quiet") {
                            println!("PASS {}  ({}B)", e.ident, full.body.len());
                        }
                    }
                    Err(err) => {
                        println!("FAIL {} 解密失败: {}", e.ident, err.msg());
                        bad += 1;
                    }
                }
            }
            println!("自检 {} 条，失败 {} 条", entries.len(), bad);
            Ok(if bad > 0 { 5 } else { 0 })
        }
        "names" => {
            for e in list_entries(&root) {
                println!("{:<40} {}", e.ident, entry_name(&e.ident));
            }
            Ok(0)
        }
        "reconcile" => {
            let (f, r, probs) = reconcile(&root);
            println!("文件 {f} / 名册 {r}");
            for p in &probs {
                println!("  - {p}");
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
                println!("[dry-run] 将导出 {} 条到 {out}", list_entries(&root).len());
                return Ok(0);
            }
            write_atomic(Path::new(&out), &s)?;
            println!("{out}");
            Ok(0)
        }
        other => usage(format!("未知 vault 子命令: {other}")),
    }
}
