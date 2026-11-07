//! Embedded MCP stdio transport for the abook executable.
//!
//! The transport is deliberately inside the one binary. MCP stdout is reserved
//! for newline-delimited JSON-RPC; CLI child output is captured and returned as
//! a tool result, never inherited by this process.

use crate::mcp_json::{self, Value};
use crate::VERSION;
use std::io::{self, BufRead, IsTerminal, Write};
use std::process::Command;

const MODERN: &str = "2026-07-28";
const LEGACY: &str = "2025-11-25";
const SERVER: &str = "abook";

pub fn is_requested(argv: &[String]) -> bool {
    argv.first().map(|x| x == "mcp").unwrap_or(false)
}

pub fn should_start_default_stdio(argv: &[String]) -> bool {
    argv.is_empty() && !io::stdin().is_terminal()
}

pub fn run_stdio() -> i32 {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout());
    let mut legacy = false;
    let mut initialized = false;
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(s) => s,
            Err(e) => { eprintln!("[abook mcp] stdin: {e}"); return 6; }
        };
        if line.trim().is_empty() { continue; }
        let parsed = match mcp_json::parse(&line) {
            Ok(v) => v,
            Err(e) => { write_error(&mut stdout, Value::Null, -32700, &e, None); continue; }
        };
        let Some(method) = parsed.str("method") else {
            write_error(&mut stdout, Value::Null, -32600, "JSON-RPC request 缺少 method", None);
            continue;
        };
        let id_present = parsed.get("id").is_some();
        let id = parsed.get("id").cloned().unwrap_or(Value::Null);
        let id_valid = matches!(id, Value::String(_) | Value::Number(_));
        if parsed.get("jsonrpc").and_then(as_string) != Some("2.0") {
            if id_present { write_error(&mut stdout, if id_valid { id } else { Value::Null }, -32600, "jsonrpc 必须为 2.0，且 id 只能是字符串或数字", None); }
            continue;
        }
        if method == "initialize" {
            let params = parsed.get("params");
            let requested = params.and_then(|p| p.str("protocolVersion")).unwrap_or("");
            if !id_present || !id_valid {
                write_error(&mut stdout, Value::Null, -32600, "initialize 必须有非空 id", None);
            } else if requested != LEGACY {
                write_error(&mut stdout, id, -32602, &format!("旧版握手只支持 {LEGACY}；现代客户端请使用 server/discover"), Some(obj(vec![("supported", Value::arr(vec![Value::s(LEGACY)])), ("requested", Value::s(requested))])));
            } else {
                legacy = true;
                initialized = false;
                write_legacy_initialize(&mut stdout, id);
            }
            continue;
        }
        if method == "notifications/initialized" {
            if legacy { initialized = true; }
            continue;
        }
        if method == "notifications/cancelled" {
            // CLI 子进程调用目前同步执行；不能谎称已经安全中断。当前无 in-flight
            // 异步任务句柄，因此该通知只记录诊断，不发送被取消请求的响应。
            eprintln!("[abook mcp] cancellation received; no asynchronous operation is running");
            continue;
        }
        if !id_present {
            // JSON-RPC notifications never receive responses.
            continue;
        }
        if !id_valid {
            write_error(&mut stdout, Value::Null, -32600, "JSON-RPC id 必须为字符串或数字，不能为 null", None);
            continue;
        }
        if legacy {
            if !initialized {
                write_error(&mut stdout, id, -32602, "legacy MCP 尚未收到 notifications/initialized", None);
                continue;
            }
        } else {
            let metadata_optional = method == "server/discover"
                && parsed.get("params").and_then(|p| p.get("_meta")).is_none();
            match if metadata_optional { Ok(MODERN.to_string()) } else { modern_meta_version(&parsed) } {
                Err(message) => { write_error(&mut stdout, id, -32602, &message, None); continue; }
                Ok(version) if version != MODERN => {
                    write_error(&mut stdout, id, -32022, &format!("不支持 MCP 协议版本 {version}"), Some(obj(vec![("supported", Value::arr(vec![Value::s(MODERN)])), ("requested", Value::s(version))])));
                    continue;
                }
                Ok(_) => {}
            }
        }
        if !legacy && method == "initialize" {
            write_error(&mut stdout, id, -32601, "现代 MCP 使用 server/discover，不使用 initialize", None);
            continue;
        }
        if legacy && method == "server/discover" {
            write_error(&mut stdout, id, -32601, "server/discover 属于现代 MCP；legacy 会话请使用 initialize 结果", None);
            continue;
        }
        let response = handle(&parsed, method, legacy);
        write_response(&mut stdout, id, legacy_result(response, legacy));
    }
    let _ = stdout.flush();
    0
}

fn handle(req: &Value, method: &str, legacy: bool) -> Value {
    match method {
        "server/discover" => discover(),
        "ping" => complete(obj(vec![])),
        "tools/list" => tools_list(legacy),
        "resources/list" => resources_list(legacy),
        "resources/templates/list" => resource_templates_list(legacy),
        "resources/read" => resources_read(req, legacy),
        "prompts/list" => prompts_list(legacy),
        "prompts/get" => prompts_get(req, legacy),
        "tools/call" => tools_call(req, legacy),
        "initialize" => complete(obj(vec![])),
        _ => error_value(-32601, &format!("未知 MCP 方法: {method}"), None),
    }
}

fn discover() -> Value {
    let mut caps = obj(vec![
        ("tools", obj(vec![])),
        ("resources", obj(vec![])),
        ("prompts", obj(vec![])),
    ]);
    let mut server_info = obj(vec![
        ("name", Value::s(SERVER)),
        ("version", Value::s(VERSION)),
    ]);
    let mut meta = obj(vec![("io.modelcontextprotocol/serverInfo", server_info.clone())]);
    let mut out = obj(vec![
        ("resultType", Value::s("complete")),
        ("supportedVersions", Value::arr(vec![Value::s(MODERN)])),
        ("capabilities", caps),
        ("_meta", meta),
        ("instructions", Value::s(instructions())),
        ("ttlMs", Value::n("3600000")),
        ("cacheScope", Value::s("private")),
    ]);
    out
}

fn instructions() -> &'static str {
    "这是当前 Agent 的结构化记忆服务。功能面包括分层记忆读写检索、规则与画像、明确记忆、长期记忆、事件维护、凭据保险箱、批量预览与应用、Skill、spec、附件、审计迁移和一键更新。遇到可能依赖历史事实时先调用 memory_context 或 memory_recall；用户明确要求记住时在本轮调用 memory_write；多条维护使用 memory_batch；多行正文使用结构化 body，不拼 shell 命令。详细参数使用 tools/list、memory_help、help 或对应 spec/skill 资源。"
}

fn tools_list(_legacy: bool) -> Value {
    let mut tools: Vec<Value> = crate::mcp_registry::public_tools().iter().map(|t| t.to_mcp_value()).collect();
    for (name, description) in [
        ("memory_help", "读取 abook 的机器可读帮助和命令描述。"),
        ("spec_list", "列出内置规范主题。"),
        ("spec_get", "读取一份内置规范全文。"),
        ("skill_list", "列出已安装 Skill。"),
        ("skill_read", "读取 Skill 正文。"),
        ("system_update", "一次调用完成更新检查、下载、校验、替换和自检。"),
    ] { tools.push(helper_tool(name, description)); }
    tools.extend(memory_tools());
    tools.extend(batch_tools());
    tools.sort_by(|a, b| a.str("name").cmp(&b.str("name")));
    complete(obj(vec![("tools", Value::arr(tools)), ("ttlMs", Value::n("300000")), ("cacheScope", Value::s("private"))]))
}

fn batch_tool(name: &str, desc: &str, properties: Vec<(&str, Value)>, required: Vec<Value>) -> Value {
    obj(vec![
        ("name", Value::s(name)),
        ("description", Value::s(desc)),
        ("inputSchema", obj(vec![("type", Value::s("object")), ("properties", obj(properties)), ("required", Value::arr(required)), ("additionalProperties", Value::b(false))])),
    ])
}

fn memory_tools() -> Vec<Value> {
    vec![
        batch_tool("memory_init", "在不包含旧布局的目录中初始化并锁定六平面记忆布局。", vec![("root", obj(vec![("type", Value::s("string"))]))], vec![Value::s("root")]),
        batch_tool("memory_context", "读取六个普通记忆平面的计数与凭据域边界。", vec![("root", obj(vec![("type", Value::s("string"))]))], vec![Value::s("root")]),
        batch_tool("memory_paths", "读取已验证六平面布局的目录路径，不暴露旧模型路径或凭据值。", vec![("root", obj(vec![("type", Value::s("string"))]))], vec![Value::s("root")]),
        batch_tool("memory_index", "分页列出六平面索引元信息，含删除统计、读取回链和标注截断的正文预览；credentials 不出值。", vec![("root", obj(vec![("type", Value::s("string"))])), ("plane", obj(vec![("type", Value::s("string")), ("enum", Value::arr(std::iter::once(Value::s("all")).chain(crate::memory_plane::PLANES.iter().map(|p| Value::s(*p))).collect()))])), ("limit", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1")), ("maximum", Value::n("500"))])), ("offset", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("0"))]))], vec![Value::s("root")]),
        batch_tool("memory_export", "全量导出普通记忆平面结构化记录；credentials 仅返回 vault 边界，不导出明文。", vec![("root", obj(vec![("type", Value::s("string"))])), ("plane", obj(vec![("type", Value::s("string")), ("enum", Value::arr(std::iter::once(Value::s("all")).chain(crate::memory_plane::PLANES.iter().map(|p| Value::s(*p))).collect()))]))], vec![Value::s("root")]),
        batch_tool("memory_write", "直接写入一个结构化普通记忆平面；credentials 走 vault。", vec![("root", obj(vec![("type", Value::s("string"))])), ("plane", obj(vec![("type", Value::s("string")), ("enum", Value::arr(crate::memory_plane::PLANES.iter().map(|p| Value::s(*p)).collect()))])), ("title", obj(vec![("type", Value::s("string"))])), ("read_when", obj(vec![("type", Value::s("string"))])), ("content", obj(vec![("type", Value::s("string"))])), ("body", obj(vec![("type", Value::s("string")), ("description", Value::s("兼容别名，优先使用 content"))]))], vec![Value::s("root"), Value::s("plane"), Value::s("title"), Value::s("content")]),
        batch_tool("memory_read", "按结构化 ID 读取一条普通记忆。", vec![("root", obj(vec![("type", Value::s("string"))])), ("id", obj(vec![("type", Value::s("string"))]))], vec![Value::s("root"), Value::s("id")]),
        batch_tool("memory_list", "分页列出普通记忆平面的活动记录；省略 plane 或传 all 时列出全部六个平面。", vec![("root", obj(vec![("type", Value::s("string"))])), ("plane", obj(vec![("type", Value::s("string")), ("enum", Value::arr(std::iter::once(Value::s("all")).chain(crate::memory_plane::PLANES.iter().map(|p| Value::s(*p))).collect()))])), ("limit", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1")), ("maximum", Value::n("500"))])), ("offset", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("0"))]))], vec![Value::s("root")]),
        batch_tool("memory_search", "在指定普通记忆平面搜索标题、正文和 read_when。", vec![("root", obj(vec![("type", Value::s("string"))])), ("query", obj(vec![("type", Value::s("string"))])), ("plane", obj(vec![("type", Value::s("string"))])), ("limit", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1"))]))], vec![Value::s("root"), Value::s("query")]),
        batch_tool("memory_recall", "memory_search 的历史事实召回入口。", vec![("root", obj(vec![("type", Value::s("string"))])), ("query", obj(vec![("type", Value::s("string"))])), ("plane", obj(vec![("type", Value::s("string"))])), ("limit", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1"))]))], vec![Value::s("root"), Value::s("query")]),
        batch_tool("memory_update", "按 expected_revision 更新一条普通记忆。", vec![("root", obj(vec![("type", Value::s("string"))])), ("id", obj(vec![("type", Value::s("string"))])), ("expected_revision", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1"))])), ("title", obj(vec![("type", Value::s("string"))])), ("read_when", obj(vec![("type", Value::s("string"))])), ("content", obj(vec![("type", Value::s("string"))])), ("body", obj(vec![("type", Value::s("string")), ("description", Value::s("兼容别名，优先使用 content"))]))], vec![Value::s("root"), Value::s("id"), Value::s("expected_revision")]),
        batch_tool("memory_delete", "按 expected_revision 软删除一条普通记忆。", vec![("root", obj(vec![("type", Value::s("string"))])), ("id", obj(vec![("type", Value::s("string"))])), ("expected_revision", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1"))]))], vec![Value::s("root"), Value::s("id"), Value::s("expected_revision")]),
        batch_tool("memory_restore", "按 expected_revision 恢复一条普通记忆。", vec![("root", obj(vec![("type", Value::s("string"))])), ("id", obj(vec![("type", Value::s("string"))])), ("expected_revision", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1"))]))], vec![Value::s("root"), Value::s("id"), Value::s("expected_revision")]),
    ]
}

fn direct_memory_call(name: &str, args: &Value) -> Value {
    let result = (|| -> Result<Value, String> {
        let allowed = match name {
            "memory_index" => &["root", "plane", "limit", "offset"][..],
            "memory_export" => &["root", "plane"][..],
            "memory_init" | "memory_paths" | "memory_context" => &["root"][..],
            "memory_write" => &["root", "plane", "title", "read_when", "content", "body"][..],
            "memory_read" => &["root", "id"][..],
            "memory_list" => &["root", "plane", "limit", "offset"][..],
            "memory_search" | "memory_recall" => &["root", "query", "plane", "limit"][..],
            "memory_update" => &[
                "root", "id", "expected_revision", "title", "read_when", "content", "body",
            ][..],
            "memory_delete" | "memory_restore" => &["root", "id", "expected_revision"][..],
            _ => return Err(format!("未知 memory 工具: {name}")),
        };
        validate_batch_args(args, allowed)?;
        let root = scalar_text(args, "root").ok_or("memory 工具缺少 root")?;
        match name {
            "memory_index" => {
                let plane = scalar_text(args, "plane");
                let limit = scalar_text(args, "limit").and_then(|s| s.parse().ok()).unwrap_or(100);
                let offset = scalar_text(args, "offset").and_then(|s| s.parse().ok()).unwrap_or(0);
                crate::memory_plane::index(&root, plane.as_deref(), limit, offset).map_err(|e| e.msg().to_string())
            }
            "memory_export" => {
                let plane = scalar_text(args, "plane");
                crate::memory_plane::export(&root, plane.as_deref()).map_err(|e| e.msg().to_string())
            }
            "memory_init" => {
                crate::memory_plane::initialize_layout(&root).map(|()| obj(vec![("layout", Value::s(crate::memory_plane::LAYOUT_MARKER))]))
                    .map_err(|e| e.msg().to_string())
            }
            "memory_paths" => crate::memory_plane::paths(&root).map_err(|e| e.msg().to_string()),
            "memory_context" => crate::memory_plane::context(&root).map_err(|e| e.msg().to_string()),
            "memory_write" => {
                let plane = scalar_text(args, "plane").ok_or("memory_write 缺少 plane")?;
                let title = scalar_text(args, "title").ok_or("memory_write 缺少 title")?;
                let content = scalar_text(args, "content").or_else(|| scalar_text(args, "body")).ok_or("memory_write 缺少 content")?;
                let read_when = scalar_text(args, "read_when").unwrap_or_default();
                crate::memory_plane::write(&root, &plane, &title, &read_when, &content).map_err(|e| e.msg().to_string())
            }
            "memory_read" => crate::memory_plane::read(&root, &scalar_text(args, "id").ok_or("memory_read 缺少 id")?).map_err(|e| e.msg().to_string()),
            "memory_list" => {
                let plane = scalar_text(args, "plane").unwrap_or_else(|| "all".to_string());
                let limit = scalar_text(args, "limit").and_then(|s| s.parse().ok()).unwrap_or(100);
                let offset = scalar_text(args, "offset").and_then(|s| s.parse().ok()).unwrap_or(0);
                crate::memory_plane::list(&root, &plane, limit, offset).map_err(|e| e.msg().to_string())
            }
            "memory_search" | "memory_recall" => {
                let query = scalar_text(args, "query").ok_or("memory_search 缺少 query")?;
                let plane = scalar_text(args, "plane");
                let limit = scalar_text(args, "limit").and_then(|s| s.parse().ok()).unwrap_or(20);
                crate::memory_plane::search(&root, &query, plane.as_deref(), limit).map_err(|e| e.msg().to_string())
            }
            "memory_update" => {
                let id = scalar_text(args, "id").ok_or("memory_update 缺少 id")?;
                let revision = scalar_text(args, "expected_revision").ok_or("memory_update 缺少 expected_revision")?;
                let content = scalar_text(args, "content").or_else(|| scalar_text(args, "body"));
                crate::memory_plane::update(&root, &id, &revision, scalar_text(args, "title").as_deref(), scalar_text(args, "read_when").as_deref(), content.as_deref()).map_err(|e| e.msg().to_string())
            }
            "memory_delete" | "memory_restore" => {
                let id = scalar_text(args, "id").ok_or("memory 工具缺少 id")?;
                let revision = scalar_text(args, "expected_revision").ok_or("memory 工具缺少 expected_revision")?;
                if name == "memory_delete" { crate::memory_plane::delete(&root, &id, &revision) } else { crate::memory_plane::restore(&root, &id, &revision) }.map_err(|e| e.msg().to_string())
            }
            _ => Err(format!("未知 memory 工具: {name}")),
        }
    })();
    match result {
        Ok(data) => complete(obj(vec![("content", Value::arr(vec![obj(vec![("type", Value::s("text")), ("text", Value::s(mcp_json::stringify(&data)))])])), ("structuredContent", data), ("isError", Value::b(false))])),
        Err(message) => complete(obj(vec![("content", Value::arr(vec![obj(vec![("type", Value::s("text")), ("text", Value::s(message))])])), ("isError", Value::b(true))])),
    }
}

fn batch_tools() -> Vec<Value> {
    vec![
        batch_tool("memory_batch_preview", "预览单批次文件变更并写入 durable preview journal，不修改目标文件。", vec![("root", obj(vec![("type", Value::s("string"))])), ("path", obj(vec![("type", Value::s("string"))])), ("body", obj(vec![("type", Value::s("string"))]))], vec![Value::s("root"), Value::s("path"), Value::s("body")]),
        batch_tool("memory_batch_apply", "按 preview token 和 expected revision 原子应用暂存变更。", vec![("root", obj(vec![("type", Value::s("string"))])), ("token", obj(vec![("type", Value::s("string"))])), ("expected_revision", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("0"))]))], vec![Value::s("root"), Value::s("token"), Value::s("expected_revision")]),
        batch_tool("memory_batch_rollback", "回滚已提交事务并推进新的库 revision。", vec![("root", obj(vec![("type", Value::s("string"))])), ("token", obj(vec![("type", Value::s("string"))]))], vec![Value::s("root"), Value::s("token")]),
        batch_tool("memory_batch_status", "读取未完成事务的恢复状态，不改目标文件。", vec![("root", obj(vec![("type", Value::s("string"))]))], vec![Value::s("root")]),
    ]
}

fn scalar_text(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|value| match value {
        Value::String(s) | Value::Number(s) => Some(s.clone()),
        _ => None,
    })
}

fn validate_batch_args(args: &Value, allowed: &[&str]) -> Result<(), String> {
    let Value::Object(map) = args else { return Err("批量工具 arguments 必须是对象".to_string()); };
    for key in map.keys() {
        if !allowed.iter().any(|candidate| candidate == key) { return Err(format!("批量工具不接受参数: {key}")); }
    }
    Ok(())
}

fn direct_batch_call(name: &str, args: &Value) -> Value {
    let result = (|| -> Result<Value, String> {
        let allowed = match name {
            "memory_batch_preview" => &["root", "path", "body"][..],
            "memory_batch_apply" => &["root", "token", "expected_revision"][..],
            "memory_batch_rollback" => &["root", "token"][..],
            "memory_batch_status" => &["root"][..],
            _ => return Err(format!("未知批量工具: {name}")),
        };
        validate_batch_args(args, allowed)?;
        let root = scalar_text(args, "root").ok_or("批量工具缺少 root")?;
        match name {
            "memory_batch_preview" => {
                let path = scalar_text(args, "path").ok_or("preview 缺少 path")?;
                let body = scalar_text(args, "body").ok_or("preview 缺少 body")?;
                let mut tx = crate::txn::Transaction::prepare(&root).map_err(|e| e.msg().to_string())?;
                tx.stage_file(crate::txn::Selector::new(path), &body).map_err(|e| e.msg().to_string())?;
                Ok(obj(vec![("token", Value::s(tx.token().to_string())), ("revision", Value::n(tx.revision.to_string()))]))
            }
            "memory_batch_apply" => {
                let token = scalar_text(args, "token").ok_or("apply 缺少 token")?;
                let expected = scalar_text(args, "expected_revision").ok_or("apply 缺少 expected_revision")?.parse::<u64>().map_err(|_| "expected_revision 必须是非负整数".to_string())?;
                let actual = crate::txn::read_library_revision(&root).map_err(|e| e.msg().to_string())?;
                if expected != actual { return Err(format!("revision 冲突：expected={expected} actual={actual}")); }
                let revision = crate::txn::apply_token(&root, &token).map_err(|e| e.msg().to_string())?;
                Ok(obj(vec![("revision", Value::n(revision.to_string()))]))
            }
            "memory_batch_rollback" => {
                let token = scalar_text(args, "token").ok_or("rollback 缺少 token")?;
                let report = crate::txn::rollback(&root, &token).map_err(|e| e.msg().to_string())?;
                Ok(obj(vec![("transaction", Value::s(report.transaction)), ("restored", Value::n(report.restored.len().to_string())), ("already_before", Value::n(report.already_before.len().to_string()))]))
            }
            "memory_batch_status" => {
                let reports = crate::txn::recover_dry_run(&root).map_err(|e| e.msg().to_string())?;
                Ok(obj(vec![("pending_transactions", Value::n(reports.len().to_string()))]))
            }
            _ => unreachable!(),
        }
    })();
    match result {
        Ok(data) => complete(obj(vec![("content", Value::arr(vec![obj(vec![("type", Value::s("text")), ("text", Value::s(mcp_json::stringify(&data)))])])), ("structuredContent", data), ("isError", Value::b(false))])),
        Err(message) => complete(obj(vec![("content", Value::arr(vec![obj(vec![("type", Value::s("text")), ("text", Value::s(message))])])), ("isError", Value::b(true))])),
    }
}

fn helper_tool(name: &str, description: &str) -> Value {
    let (properties, required): (Vec<(&str, Value)>, Vec<Value>) = match name {
        "spec_get" => (vec![("name", obj(vec![("type", Value::s("string"))]))], vec![Value::s("name")]),
        "skill_list" => (vec![("root", obj(vec![("type", Value::s("string"))]))], vec![]),
        "skill_read" => (vec![("name", obj(vec![("type", Value::s("string"))])), ("root", obj(vec![("type", Value::s("string"))]))], vec![Value::s("name")]),
        _ => (vec![], vec![]),
    };
    batch_tool(name, description, properties, required)
}

fn validate_helper_args(name: &str, args: &Value) -> Result<(), String> {
    let Value::Object(values) = args else { return Err("helper 工具 arguments 必须是对象".into()); };
    let allowed: &[&str] = match name {
        "spec_get" => &["name"],
        "skill_list" => &["root"],
        "skill_read" => &["name", "root"],
        "memory_help" | "spec_list" | "system_update" => &[],
        _ => return Err(format!("未知 helper 工具: {name}")),
    };
    for key in values.keys() {
        if !allowed.contains(&key.as_str()) { return Err(format!("工具参数未声明: {key}")); }
    }
    if matches!(name, "spec_get" | "skill_read") && values.get("name").and_then(as_string).filter(|s| !s.is_empty()).is_none() {
        return Err(format!("{name} 缺少必填参数 name"));
    }
    for key in ["name", "root"] {
        if let Some(value) = values.get(key) {
            if as_string(value).is_none() { return Err(format!("工具参数 {key} 必须是字符串")); }
        }
    }
    Ok(())
}

fn resources_list(_legacy: bool) -> Value {
    complete(obj(vec![
        ("resources", Value::arr(vec![
            resource("abook://help", "abook-help", "text/plain"),
            resource("abook://state", "memory-state", "application/json"),
            resource("abook://instructions", "memory-instructions", "text/plain"),
        ])),
        ("ttlMs", Value::n("60000")),
        ("cacheScope", Value::s("private")),
    ]))
}

fn resource_templates_list(_legacy: bool) -> Value {
    complete(obj(vec![
        ("resourceTemplates", Value::arr(vec![
            obj(vec![("uriTemplate", Value::s("abook://spec/{name}")), ("name", Value::s("spec")), ("description", Value::s("读取指定内置规范")), ("mimeType", Value::s("text/markdown"))]),
            obj(vec![("uriTemplate", Value::s("abook://skill/{name}")), ("name", Value::s("skill")), ("description", Value::s("读取指定 Skill")), ("mimeType", Value::s("text/markdown"))]),
        ])),
        ("ttlMs", Value::n("600000")),
        ("cacheScope", Value::s("private")),
    ]))
}

fn resource(uri: &str, name: &str, mime: &str) -> Value {
    obj(vec![("uri", Value::s(uri)), ("name", Value::s(name)), ("mimeType", Value::s(mime))])
}

fn resources_read(req: &Value, _legacy: bool) -> Value {
    let uri = req.get("params").and_then(|p| p.str("uri")).unwrap_or("");
    let cli_text = |argv: Vec<String>, mime: &str| {
        let (stdout, stderr, code) = run_cli_text(&argv, None);
        if code == 0 {
            Ok((stdout, mime.to_string()))
        } else {
            let detail = if stderr.trim().is_empty() { stdout } else { stderr };
            Err(error_value(-32602, &format!("读取资源失败（exit={code}）：{}", detail.trim()), None))
        }
    };
    let loaded: Result<(String, String), Value> = if uri == "abook://instructions" { Ok((instructions().to_string(), "text/plain".into())) }
    else if uri == "abook://help" { Ok((crate::help_text_public(), "text/plain".into())) }
    else if uri == "abook://state" { cli_text(vec!["state".into(), "--json".into()], "application/json") }
    else if let Some(name) = uri.strip_prefix("abook://spec/") {
        if !valid_resource_segment(name) { return error_value(-32602, "非法 spec 资源名", None); }
        cli_text(vec!["spec".into(), name.into(), "--json".into()], "text/markdown")
    }
    else if let Some(name) = uri.strip_prefix("abook://skill/") {
        if !valid_resource_segment(name) { return error_value(-32602, "非法 Skill 资源名", None); }
        cli_text(vec!["skill".into(), "show".into(), name.into(), "--json".into()], "text/markdown")
    }
    else { return error_value(-32602, &format!("资源不存在: {uri}"), None); };
    match loaded {
        Ok((text, mime)) => complete(obj(vec![
            ("contents", Value::arr(vec![obj(vec![("uri", Value::s(uri)), ("mimeType", Value::s(mime)), ("text", Value::s(text))])])),
            ("ttlMs", Value::n("60000")), ("cacheScope", Value::s("private")),
        ])),
        Err(error) => error,
    }
}

fn valid_resource_segment(s: &str) -> bool {
    !s.is_empty() && !s.contains('/') && !s.contains('\\') && !s.contains("..") && !s.chars().any(char::is_control)
}

fn prompts_list(_legacy: bool) -> Value {
    let names = [
        ("memory-first", "在回答依赖历史信息时先读取记忆"),
        ("memory-write", "把明确的记忆请求写入指定平面"),
        ("memory-maintenance", "批量预览、应用和核对记忆维护"),
        ("skill-authoring", "读取 Skill 规范并进行 Skill 工作"),
        ("spec-lookup", "按主题读取内置规范"),
        ("system-update", "通过一次工具调用完成系统更新"),
    ];
    complete(obj(vec![
        ("prompts", Value::arr(names.iter().map(|(n, d)| obj(vec![("name", Value::s(*n)), ("description", Value::s(*d)), ("arguments", Value::arr(vec![]))])).collect())),
        ("ttlMs", Value::n("600000")),
        ("cacheScope", Value::s("public")),
    ]))
}

fn prompts_get(req: &Value, _legacy: bool) -> Value {
    let name = req.get("params").and_then(|p| p.str("name")).unwrap_or("");
    let text = match name {
        "memory-first" => "当回答可能依赖历史事实、偏好、项目状态或既有经验时，先调用 memory_context 或 memory_recall，再回答。",
        "memory-write" => "用户明确要求记住、保存、以后记得、加入画像或作为规则时，在本轮调用 memory_write。",
        "memory-maintenance" => "一次维护多条记录使用 memory_batch_preview 和 memory_batch_apply，不逐条拼接 shell 命令。",
        "skill-authoring" => "先调用 skill_list/skill_read，再读取 skill 规范并执行 Skill 工作。",
        "spec-lookup" => "需要规则全文时调用 spec_list/spec_get，不在提示词中猜规范。",
        "system-update" => "调用 system_update 完成检查、下载、校验、替换、自检和接管。",
        _ => return error_value(-32602, "未知 prompt", None),
    };
    complete(obj(vec![
        ("description", Value::s("abook prompt")),
        ("messages", Value::arr(vec![obj(vec![
            ("role", Value::s("user")),
            ("content", obj(vec![("type", Value::s("text")), ("text", Value::s(text))])),
        ])])),
    ]))
}

fn tools_call(req: &Value, _legacy: bool) -> Value {
    let params = req.get("params");
    let name = params.and_then(|p| p.str("name")).unwrap_or("");
    if !tool_is_registered(name) {
        return error_value(-32602, &format!("未知工具: {name}"), None);
    }
    let args = params.and_then(|p| p.get("arguments"));
    if matches!(name, "memory_init" | "memory_paths" | "memory_index" | "memory_export" | "memory_context" | "memory_write" | "memory_read" | "memory_list" | "memory_search" | "memory_recall" | "memory_update" | "memory_delete" | "memory_restore") {
        return direct_memory_call(name, args.unwrap_or(&obj(vec![])));
    }
    if matches!(name, "memory_batch_preview" | "memory_batch_apply" | "memory_batch_rollback" | "memory_batch_status") {
        return direct_batch_call(name, args.unwrap_or(&obj(vec![])));
    }
    let (argv, body) = match tool_argv(name, args) {
        Ok(v) => v,
        Err(e) => return error_value(-32602, &e, None),
    };
    let (out, err, code) = run_cli_text(&argv, body.as_deref());
    let mut result = obj(vec![
        ("content", Value::arr(vec![obj(vec![("type", Value::s("text")), ("text", Value::s(if out.is_empty() { err.clone() } else { out.clone() }))])])),
        ("isError", Value::b(code != 0)),
    ]);
    if let Value::Object(ref mut m) = result {
        m.insert("structuredContent".into(), mcp_json::parse(&out).unwrap_or_else(|_| obj(vec![("stdout", Value::s(out)), ("stderr", Value::s(err)), ("exit", Value::n(code.to_string()))])));
    }
    complete(result)
}

fn legacy_readonly_command(name: &str) -> bool {
    !matches!(name, "write" | "add" | "promote" | "update" | "edit" | "delete" | "rm" | "restore" | "set" | "unset" | "append" | "prepend" | "patch" | "rename" | "move" | "mv" | "copy" | "cp" | "demote" | "merge" | "split" | "repair" | "fix" | "init" | "refresh" | "sync" | "normalize" | "commit" | "branch" | "recount" | "leftover" | "vault" | "install" | "self-install" | "upgrade" | "self-update")
}

fn tool_is_registered(name: &str) -> bool {
    if crate::mcp_registry::public_tools().iter().any(|tool| tool.name == name && tool.is_public()) { return true; }
    matches!(name, "memory_init" | "memory_paths" | "memory_index" | "memory_export" | "memory_context" | "memory_write" | "memory_read" | "memory_list" | "memory_search" | "memory_recall" | "memory_update" | "memory_delete" | "memory_restore" | "memory_batch_preview" | "memory_batch_apply" | "memory_batch_rollback" | "memory_batch_status" | "memory_help" | "spec_list" | "spec_get" | "skill_list" | "skill_read" | "system_update")
}

fn tool_argv(name: &str, args: Option<&Value>) -> Result<(Vec<String>, Option<String>), String> {
    let a = args.cloned().unwrap_or_else(|| obj(vec![]));
    if matches!(name, "memory_help" | "spec_list" | "spec_get" | "skill_list" | "skill_read" | "system_update") {
        validate_helper_args(name, &a)?;
    }
    if name == "memory_help" { return Ok((vec!["help".into()], None)); }
    if name == "spec_list" { return Ok((vec!["spec".into(), "--json".into()], None)); }
    if name == "spec_get" { return Ok((vec!["spec".into(), a.str("name").ok_or("spec_get 缺少 name")?.into(), "--json".into()], None)); }
    if name == "skill_list" {
        let mut argv = vec!["skill".into(), "list".into(), "--json".into()];
        if let Some(root) = a.str("root") { argv.extend(["--root".into(), root.into()]); }
        return Ok((argv, None));
    }
    if name == "skill_read" {
        let mut argv = vec!["skill".into(), "show".into(), a.str("name").ok_or("skill_read 缺少 name")?.into(), "--json".into()];
        if let Some(root) = a.str("root") { argv.extend(["--root".into(), root.into()]); }
        return Ok((argv, None));
    }
    if name == "system_update" { return Ok((vec!["upgrade".into(), "--json".into()], None)); }
    if name.starts_with("abook_") {
        return match crate::mcp_registry::build_argv(name, &a) {
            Ok(argv) => Ok((argv, None)),
            Err(e) => Err(e),
        };
    }
    Err(format!("未知工具: {name}"))
}

fn run_cli_text(argv: &[String], body: Option<&str>) -> (String, String, i32) {
    let exe = match crate::util::self_path() {
        Some(p) if p.is_file() => p,
        _ => return (String::new(), "无法定位 abook 可执行文件".into(), 6),
    };
    let mut actual = argv.to_vec();
    let temp = if let Some(text) = body {
        match write_body_temp(text) {
            Ok(p) => {
                actual.push("--body-file".into());
                actual.push(p.to_string_lossy().to_string());
                Some(p)
            }
            Err(e) => return (String::new(), format!("无法建立 MCP 正文临时文件：{e}"), 6),
        }
    } else { None };
    let result = Command::new(exe).args(&actual).env("ABOOK_NO_GUARD", "1").output();
    let cleanup_error = temp.and_then(|p| std::fs::remove_file(p).err());
    match result {
        Ok(o) => {
            let mut err = String::from_utf8_lossy(&o.stderr).into_owned();
            if let Some(e) = cleanup_error { err.push_str(&format!("\n无法删除正文临时文件：{e}")); }
            (String::from_utf8_lossy(&o.stdout).into_owned(), err, o.status.code().unwrap_or(1))
        }
        Err(e) => {
            let mut message = e.to_string();
            if let Some(c) = cleanup_error { message.push_str(&format!("；清理正文临时文件也失败：{c}")); }
            (String::new(), message, 6)
        }
    }
}

fn write_body_temp(text: &str) -> std::io::Result<std::path::PathBuf> {
    use std::fs::OpenOptions;
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir();
    for _ in 0..32 {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("abook-mcp-body-{}-{serial}.tmp", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(text.as_bytes()) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(e);
                }
                file.flush()?;
                drop(file);
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "临时文件名冲突重试达到上限"))
}
fn as_string(v: &Value) -> Option<&str> { match v { Value::String(s) => Some(s), _ => None } }
fn complete(v: Value) -> Value {
    match v {
        Value::Object(mut m) => {
            m.entry("resultType".into()).or_insert_with(|| Value::s("complete"));
            Value::Object(m)
        }
        other => other,
    }
}
fn obj(items: Vec<(&str, Value)>) -> Value { Value::Object(items.into_iter().map(|(k, v)| (k.to_string(), v)).collect()) }
fn error_value(code: i64, message: &str, data: Option<Value>) -> Value { obj(vec![("__error", Value::n(code.to_string())), ("message", Value::s(message)), ("data", data.unwrap_or(Value::Null))]) }
fn modern_meta_version(req: &Value) -> Result<String, String> {
    let Some(m) = req.get("params").and_then(|p| p.get("_meta")) else {
        return Err("现代 MCP 请求缺少 params._meta".into());
    };
    let Some(version) = m.str("io.modelcontextprotocol/protocolVersion") else {
        return Err("_meta 缺少 io.modelcontextprotocol/protocolVersion".into());
    };
    if !matches!(m.get("io.modelcontextprotocol/clientCapabilities"), Some(Value::Object(_))) {
        return Err("_meta 缺少 io.modelcontextprotocol/clientCapabilities 对象".into());
    }
    Ok(version.to_string())
}

fn legacy_result(v: Value, legacy: bool) -> Value {
    if !legacy { return v; }
    match v {
        Value::Object(mut m) => {
            m.remove("resultType");
            m.remove("ttlMs");
            m.remove("cacheScope");
            Value::Object(m)
        }
        other => other,
    }
}

fn write_response(w: &mut impl Write, id: Value, result: Value) { let line = if let Value::Object(mut m) = result { if let Some(Value::Number(code)) = m.remove("__error") { let message = m.remove("message").unwrap_or(Value::s("error")); let data = m.remove("data").unwrap_or(Value::Null); obj(vec![("jsonrpc", Value::s("2.0")), ("id", id), ("error", obj(vec![("code", Value::Number(code)), ("message", message), ("data", data)]))]) } else { obj(vec![("jsonrpc", Value::s("2.0")), ("id", id), ("result", Value::Object(m))]) } } else { obj(vec![("jsonrpc", Value::s("2.0")), ("id", id), ("result", result)]) }; let _ = writeln!(w, "{}", mcp_json::stringify(&line)); let _ = w.flush(); }
fn write_error(w: &mut impl Write, id: Value, code: i64, message: &str, data: Option<Value>) { write_response(w, id, error_value(code, message, data)); }
fn write_legacy_initialize(w: &mut impl Write, id: Value) { let r = obj(vec![("protocolVersion", Value::s(LEGACY)), ("capabilities", obj(vec![("tools", obj(vec![])), ("resources", obj(vec![])), ("prompts", obj(vec![]))])), ("serverInfo", obj(vec![("name", Value::s(SERVER)), ("version", Value::s(VERSION))])), ("instructions", Value::s(instructions()))]); let msg = obj(vec![("jsonrpc", Value::s("2.0")), ("id", id), ("result", r)]); let _ = writeln!(w, "{}", mcp_json::stringify(&msg)); let _ = w.flush(); }
