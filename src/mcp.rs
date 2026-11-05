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
            match modern_meta_version(&parsed) {
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
        ("supportedVersions", Value::arr(vec![Value::s(MODERN), Value::s(LEGACY)])),
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
    let mut tools = Vec::new();
    for name in crate::known_commands() {
        if name.starts_with('-') { continue; }
        tools.push(tool(&format!("abook_{name}"), &format!("调用 abook CLI 命令 `{name}`；参数使用结构化 argv 数组，不经过 shell。")));
    }
    for (name, desc) in [
        ("memory_help", "读取 abook 的机器可读帮助和命令描述。"),
        ("spec_list", "列出内置规范主题。"),
        ("spec_get", "读取一份内置规范全文。"),
        ("skill_list", "列出已安装 Skill。"),
        ("skill_read", "读取 Skill。"),
        ("system_update", "一次调用完成更新检查、下载、校验、替换和自检。"),
    ] { tools.push(tool(name, desc)); }
    tools.sort_by(|a, b| a.str("name").cmp(&b.str("name")));
    complete(obj(vec![
        ("tools", Value::arr(tools)),
        ("ttlMs", Value::n("300000")),
        ("cacheScope", Value::s("private")),
    ]))
}

fn tool(name: &str, description: &str) -> Value {
    obj(vec![
        ("name", Value::s(name)),
        ("description", Value::s(description)),
        ("inputSchema", obj(vec![
            ("type", Value::s("object")),
            ("properties", obj(vec![
                ("argv", obj(vec![("type", Value::s("array")), ("items", obj(vec![("type", Value::s("string"))]))])),
                ("root", obj(vec![("type", Value::s("string"))])),
            ])),
        ])),
    ])
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

fn resources_read(req: &Value, legacy: bool) -> Value {
    let uri = req.get("params").and_then(|p| p.str("uri")).unwrap_or("");
    let text = if uri == "abook://instructions" { Some(instructions().to_string()) }
    else if uri == "abook://help" { Some(crate::help_text_public()) }
    else if uri == "abook://state" { Some(run_cli_text(&vec!["state".into(), "--json".into()], None).0) }
    else if let Some(name) = uri.strip_prefix("abook://spec/") { Some(run_cli_text(&vec!["spec".into(), name.into(), "--json".into()], None).0) }
    else { None };
    match text {
        Some(t) => complete(obj(vec![
            ("contents", Value::arr(vec![obj(vec![("uri", Value::s(uri)), ("mimeType", Value::s("text/plain")), ("text", Value::s(t))])])),
            ("ttlMs", Value::n("60000")),
            ("cacheScope", Value::s("private")),
        ])),
        None => error_value(-32602, &format!("资源不存在: {uri}"), None),
    }
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
    let args = params.and_then(|p| p.get("arguments"));
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

fn tool_argv(name: &str, args: Option<&Value>) -> Result<(Vec<String>, Option<String>), String> {
    let a = args.cloned().unwrap_or_else(|| obj(vec![]));
    if name == "memory_context" { return Ok((vec!["index".into(), "--json".into()], None)); }
    if name == "memory_write" {
        let title = a.str("title").ok_or("memory_write 缺少 title")?;
        let read_when = a.str("read_when").unwrap_or("");
        let body = a.str("body").ok_or("memory_write 缺少 body")?.to_string();
        let plane = a.str("plane").unwrap_or("episodic");
        return Ok((vec!["write".into(), "--title".into(), title.into(), "--read-when".into(), read_when.into(), "--lib".into(), plane.into(), "--json".into()], Some(body)));
    }
    if name == "memory_read" { return Ok((vec!["show".into(), a.str("id").ok_or("memory_read 缺少 id")?.into(), "--json".into()], None)); }
    if name == "memory_search" { return Ok((vec!["search".into(), a.str("query").ok_or("memory_search 缺少 query")?.into(), "--json".into()], None)); }
    if name == "memory_recall" { return Ok((vec!["recall".into(), a.str("query").ok_or("memory_recall 缺少 query")?.into(), "--json".into()], None)); }
    if name == "memory_batch_preview" || name == "memory_batch_apply" || name == "memory_batch_rollback" || name == "memory_batch_status" {
        return Err(format!("工具尚未实现事务内核: {name}"));
    }
    if name == "memory_help" { return Ok((vec!["help".into()], None)); }
    if name == "spec_list" { return Ok((vec!["spec".into(), "--json".into()], None)); }
    if name == "spec_get" { return Ok((vec!["spec".into(), a.str("name").ok_or("spec_get 缺少 name")?.into(), "--json".into()], None)); }
    if name == "skill_list" { return Ok((vec!["skill".into(), "list".into(), "--json".into()], None)); }
    if name == "skill_read" { return Ok((vec!["skill".into(), "read".into(), a.str("name").ok_or("skill_read 缺少 name")?.into(), "--json".into()], None)); }
    if name == "system_update" { return Ok((vec!["upgrade".into(), "--json".into()], None)); }
    let command = name.strip_prefix("abook_").ok_or_else(|| format!("未知工具: {name}"))?;
    if !crate::known_commands().contains(&command) { return Err(format!("未知工具: {name}")); }
    let mut out = vec![command.to_string()];
    if let Some(argv) = a.array("argv") {
        for item in argv { out.push(match item { Value::String(s) => s.clone(), _ => return Err("argv 只能包含字符串".into()) }); }
    }
    if let Some(root) = a.str("root") { out.push("--root".into()); out.push(root.into()); }
    if !out.iter().any(|x| x == "--json") { out.push("--json".into()); }
    Ok((out, None))
}

fn run_cli_text(argv: &[String], body: Option<&str>) -> (String, String, i32) {
    let exe = match std::env::current_exe() { Ok(p) => p, Err(e) => return (String::new(), e.to_string(), 6) };
    let mut actual = argv.to_vec();
    let root = actual.iter().position(|x| x == "--root").and_then(|i| actual.get(i + 1)).cloned();
    let temp = if let Some(text) = body {
        let dir = root.as_ref().map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let p = dir.join(format!(".abook-mcp-body-{}", std::process::id()));
        if let Err(e) = std::fs::write(&p, text) { return (String::new(), e.to_string(), 6); }
        actual.push("--body-file".into()); actual.push(p.to_string_lossy().to_string()); Some(p)
    } else { None };
    let result = Command::new(exe).args(&actual).env("ABOOK_NO_GUARD", "1").output();
    if let Some(p) = temp { let _ = std::fs::remove_file(p); }
    match result {
        Ok(o) => (String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned(), o.status.code().unwrap_or(1)),
        Err(e) => (String::new(), e.to_string(), 6),
    }
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
