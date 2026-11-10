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

/// 进程级固定库根。
///
/// MCP 服务由客户端在配置里拉起，客户端本来就能给启动参数。把库根在**配置期**
/// 定下来（`abook mcp stdio --root /path/to/lib`），比每次调用时推导更确定：
/// 推导依赖二进制位置、环境变量和 cwd，任何一处变动都可能指向别的库；
/// 而写进配置的路径是唯一的、可复核的、不随运行环境漂移的。
///
/// 进程内所有未显式传 root 的工具调用都用它，显式传的仍然优先。
static ATTACHED_ROOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn attached_root() -> Option<&'static str> {
    ATTACHED_ROOT.get().map(String::as_str)
}

/// 从 MCP 启动参数里取 `--root` / `--root=...`。
fn parse_attached_root(args: &[String]) -> Option<String> {
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some(value) = arg.strip_prefix("--root=") {
            if !value.trim().is_empty() { return Some(value.trim().to_string()); }
        }
        if arg == "--root" {
            if let Some(value) = args.get(index + 1) {
                if !value.starts_with('-') && !value.trim().is_empty() {
                    return Some(value.trim().to_string());
                }
            }
        }
        index += 1;
    }
    None
}

/// 共享 stdout：并发工具调用各自持锁写完整条响应。
type SharedStdout = std::sync::Arc<std::sync::Mutex<io::BufWriter<io::Stdout>>>;

pub fn run_stdio(argv: &[String]) -> i32 {
    if let Some(root) = parse_attached_root(argv) {
        let _ = ATTACHED_ROOT.set(root);
    }
    let stdin = io::stdin();
    // 并发 `tools/call` 会把响应从各自的线程写回来，所以 stdout 必须是共享的、
    // 且每次整条响应在锁内写完——否则两个线程的行会交错，客户端看到的就是
    // 一条被劈成两半的 JSON。
    let stdout = std::sync::Arc::new(std::sync::Mutex::new(io::BufWriter::new(io::stdout())));
    // 在途子任务计数：stdin 关了之后，还在跑的工具调用可能仍要写响应。
    // 不等它们写完就返回，客户端会看到"进程退了但没收到结果"。
    let inflight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
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
            Err(e) => { emit_error(&stdout, Value::Null, -32700, &e, None); continue; }
        };
        let Some(method) = parsed.str("method") else {
            emit_error(&stdout, Value::Null, -32600, "JSON-RPC request 缺少 method", None);
            continue;
        };
        let id_present = parsed.get("id").is_some();
        let id = parsed.get("id").cloned().unwrap_or(Value::Null);
        let id_valid = matches!(id, Value::String(_) | Value::Number(_));
        if parsed.get("jsonrpc").and_then(as_string) != Some("2.0") {
            if id_present { emit_error(&stdout, if id_valid { id } else { Value::Null }, -32600, "jsonrpc 必须为 2.0，且 id 只能是字符串或数字", None); }
            continue;
        }
        if method == "initialize" {
            let params = parsed.get("params");
            let requested = params.and_then(|p| p.str("protocolVersion")).unwrap_or("");
            if !id_present || !id_valid {
                emit_error(&stdout, Value::Null, -32600, "initialize 必须有非空 id", None);
            } else if requested != LEGACY {
                emit_error(&stdout, id, -32602, &format!("旧版握手只支持 {LEGACY}；现代客户端请使用 server/discover"), Some(obj(vec![("supported", Value::arr(vec![Value::s(LEGACY)])), ("requested", Value::s(requested))])));
            } else {
                legacy = true;
                initialized = false;
                emit_legacy_initialize(&stdout, id);
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
            emit_error(&stdout, Value::Null, -32600, "JSON-RPC id 必须为字符串或数字，不能为 null", None);
            continue;
        }
        if legacy {
            if !initialized {
                emit_error(&stdout, id, -32602, "legacy MCP 尚未收到 notifications/initialized", None);
                continue;
            }
        } else {
            let metadata_optional = method == "server/discover"
                && parsed.get("params").and_then(|p| p.get("_meta")).is_none();
            match if metadata_optional { Ok(MODERN.to_string()) } else { modern_meta_version(&parsed) } {
                Err(message) => { emit_error(&stdout, id, -32602, &message, None); continue; }
                Ok(version) if version != MODERN => {
                    emit_error(&stdout, id, -32022, &format!("不支持 MCP 协议版本 {version}"), Some(obj(vec![("supported", Value::arr(vec![Value::s(MODERN)])), ("requested", Value::s(version))])));
                    continue;
                }
                Ok(_) => {}
            }
        }
        if !legacy && method == "initialize" {
            emit_error(&stdout, id, -32601, "现代 MCP 使用 server/discover，不使用 initialize", None);
            continue;
        }
        if legacy && method == "server/discover" {
            emit_error(&stdout, id, -32601, "server/discover 属于现代 MCP；legacy 会话请使用 initialize 结果", None);
            continue;
        }
        // `tools/call` 会 fork 子进程跑 CLI，单个调用可能就是几百毫秒。串行处理
        // 会让六个会话排成一队，其中一个跑长任务时其余全卡住。所以把工具调用
        // 派到独立线程，主循环立刻回去读下一行。
        //
        // 其余方法（initialize/discover/tools/list…）都是纯内存构造，快到一个
        // 线程的调度开销比执行本身还贵，留在主循环里同步做。
        // `tools/call` 会 fork 子进程跑 CLI，单个调用可能几百毫秒起步。
        // 串行处理时六个会话会排成一队：一个跑长任务，其余全卡住。
        // 派到独立线程后主循环立刻回去读下一行，响应由各自的线程持锁写回。
        //
        // 其余方法（initialize/discover/tools/list…）都是纯内存构造，快到
        // 一个线程的调度开销比执行本身还贵，留在主循环里同步做。
        if method == "tools/call" {
            let stdout = std::sync::Arc::clone(&stdout);
            let inflight = std::sync::Arc::clone(&inflight);
            let req = parsed.clone();
            inflight.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::thread::spawn(move || {
                let response = handle(&req, "tools/call", legacy);
                emit(&stdout, id, response, legacy);
                inflight.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            });
            continue;
        }
        let response = handle(&parsed, method, legacy);
        emit(&stdout, id, response, legacy);
    }
    // stdin 关了，但在途的工具调用可能还在写响应。等它们写完再退出。
    let mut waited = 0u32;
    while inflight.load(std::sync::atomic::Ordering::SeqCst) > 0 && waited < 60_000 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 10;
    }
    {
        let mut guard = match stdout.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let _ = guard.flush();
    }
    0
}


/// 并发安全的响应写出：整条响应在同一把锁内写完。
///
/// 拆成"取锁→格式化→写→flush"四步而不是只锁 flush，是因为交错发生在行中间：
/// 两个线程各写半行，客户端拿到的是两行都不合法的 JSON。
fn emit_legacy_initialize(stdout: &SharedStdout, id: Value) {
    let mut guard = match stdout.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    write_legacy_initialize(&mut *guard, id);
    let _ = guard.flush();
}

/// 写一条成功响应。`legacy` 决定是否剥掉现代协议字段——它必须在这里生效，
/// 因为调用方传进来的是原始 result，剥壳是写出前最后一步。
fn emit(stdout: &SharedStdout, id: Value, result: Value, legacy: bool) {
    let payload = legacy_result(result, legacy);
    let mut guard = match stdout.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    write_response(&mut *guard, id, payload);
    let _ = guard.flush();
}

fn emit_error(stdout: &SharedStdout, id: Value, code: i64, message: &str, data: Option<Value>) {
    let mut guard = match stdout.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    write_error(&mut *guard, id, code, message, data);
    let _ = guard.flush();
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
    "这是当前 Agent 的结构化记忆服务。普通记忆平面为 soul、rules、profile、explicit、long_term、episodic；credentials 由独立 vault 存储。功能面包括分层记忆读写检索、批量预览与应用、Skill、spec、附件、审计迁移和一键更新。\n\
     优先用这里提供的工具：凡本服务已把某项能力暴露为工具，任务与执行就走工具调用，不要退回命令行 `abook`。命令行只用于工具未覆盖的场合、以及历史库兼容与迁移。同一件事有工具也有命令时，工具是默认路径。\n\
     如果当前环境还没有接入本服务的 MCP，那么拿到可执行文件之后紧接着要做的第一件事就是为本环境配置它，配好再开始干活，不要用命令行代跑完事。配置时把记忆库根一并写进启动参数——`args` 里给出 `mcp stdio --root <库根>`——这样库定位在配置期就固定下来，之后每次工具调用都不必再传 root，也不会因为运行目录或环境变量变化而指到别的库。库根用 `detect` 或 `memory_paths` 查一次即可。怎么配取决于你所在的环境：宿主文档、现有配置文件和你的探查都能告诉你答案，按当前环境的实际机制配，不必套用别的宿主的写法。\n\
     遇到可能依赖历史事实时先调用 memory_context 或 memory_recall；用户明确要求记住时在本轮调用 memory_write；多条维护使用 memory_batch；多行正文使用结构化 body，不拼 shell 命令。新 MCP 直接维护平面记录，不使用 candidate/canonical 晋升生命周期。详细参数使用 tools/list、memory_help、help 或对应 spec/skill 资源。"
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
        batch_tool("memory_init", "在不包含旧布局的目录中初始化并锁定六平面记忆布局。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))]))], vec![]),
        batch_tool("memory_context", "读取六个普通记忆平面的计数与凭据域边界。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))]))], vec![]),
        batch_tool("memory_paths", "读取已验证六平面布局的目录路径，不暴露旧模型路径或凭据值。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))]))], vec![]),
        batch_tool("memory_index", "分页列出六平面索引元信息，含删除统计、读取回链和标注截断的正文预览；credentials 不出值。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))])), ("plane", obj(vec![("type", Value::s("string")), ("enum", Value::arr(std::iter::once(Value::s("all")).chain(crate::memory_plane::PLANES.iter().map(|p| Value::s(*p))).collect()))])), ("limit", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1")), ("maximum", Value::n("500"))])), ("offset", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("0"))]))], vec![]),
        batch_tool("memory_export", "全量导出普通记忆平面结构化记录；credentials 仅返回 vault 边界，不导出明文。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))])), ("plane", obj(vec![("type", Value::s("string")), ("enum", Value::arr(std::iter::once(Value::s("all")).chain(crate::memory_plane::PLANES.iter().map(|p| Value::s(*p))).collect()))]))], vec![]),
        batch_tool("memory_write", "直接写入一个结构化普通记忆平面；credentials 走 vault。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))])), ("plane", obj(vec![("type", Value::s("string")), ("enum", Value::arr(crate::memory_plane::PLANES.iter().map(|p| Value::s(*p)).collect()))])), ("title", obj(vec![("type", Value::s("string"))])), ("read_when", obj(vec![("type", Value::s("string"))])), ("content", obj(vec![("type", Value::s("string"))])), ("body", obj(vec![("type", Value::s("string")), ("description", Value::s("兼容别名，优先使用 content"))]))], vec![Value::s("plane"), Value::s("title"), Value::s("content")]),
        batch_tool("memory_read", "按结构化 ID 读取一条普通记忆。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))])), ("id", obj(vec![("type", Value::s("string"))]))], vec![Value::s("id")]),
        batch_tool("memory_list", "分页列出普通记忆平面的活动记录；省略 plane 或传 all 时列出全部六个平面。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))])), ("plane", obj(vec![("type", Value::s("string")), ("enum", Value::arr(std::iter::once(Value::s("all")).chain(crate::memory_plane::PLANES.iter().map(|p| Value::s(*p))).collect()))])), ("limit", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1")), ("maximum", Value::n("500"))])), ("offset", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("0"))])), ("status", obj(vec![("type", Value::s("string")), ("description", Value::s("只看该状态的记录：active 或 deleted"))])), ("kind", obj(vec![("type", Value::s("string")), ("description", Value::s("只看该 KIND 的记录，如 DECISION"))])), ("include_deleted", obj(vec![("type", Value::s("boolean")), ("description", Value::s("true 时把软删记录一并列出，可当回收站用"))])), ("full", obj(vec![("type", Value::s("boolean")), ("description", Value::s("true 时返回完整记录；默认只给 id/title/plane/revision/preview 等定位字段，省约 85% 体积"))]))], vec![]),
        batch_tool("memory_search", "在指定普通记忆平面搜索标题、正文和 read_when。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))])), ("query", obj(vec![("type", Value::s("string"))])), ("plane", obj(vec![("type", Value::s("string"))])), ("limit", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1"))])), ("field", obj(vec![("type", Value::s("string")), ("description", Value::s("限定检索字段：all/title/content/read-when"))])), ("regex", obj(vec![("type", Value::s("boolean")), ("description", Value::s("true 时把 query 当正则"))])), ("full", obj(vec![("type", Value::s("boolean")), ("description", Value::s("true 时返回完整记录；默认只给 id/title/plane/revision/preview，省约 85% 体积"))]))], vec![Value::s("query")]),
        batch_tool("memory_recall", "memory_search 的历史事实召回入口。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))])), ("query", obj(vec![("type", Value::s("string"))])), ("plane", obj(vec![("type", Value::s("string"))])), ("limit", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1"))]))], vec![Value::s("query")]),
        batch_tool("memory_update", "按 expected_revision 更新一条普通记忆。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时自动推导"))])), ("id", obj(vec![("type", Value::s("string"))])), ("expected_revision", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1"))])), ("title", obj(vec![("type", Value::s("string"))])), ("read_when", obj(vec![("type", Value::s("string"))])), ("content", obj(vec![("type", Value::s("string"))])), ("body", obj(vec![("type", Value::s("string")), ("description", Value::s("兼容别名，优先使用 content"))])), ("fields", obj(vec![("type", Value::s("object")), ("description", Value::s("头部字段键值对对象，数组值按逗号拼接"))]))], vec![Value::s("id"), Value::s("expected_revision")]),
        batch_tool("memory_delete", "按 expected_revision 软删除一条普通记忆。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))])), ("id", obj(vec![("type", Value::s("string"))])), ("expected_revision", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1"))]))], vec![Value::s("id"), Value::s("expected_revision")]),
        batch_tool("memory_move_plane", "按 expected_revision 把一条记录移到另一个平面，保留 ID 与关系链接。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时自动推导"))])), ("id", obj(vec![("type", Value::s("string"))])), ("to_plane", obj(vec![("type", Value::s("string")), ("enum", Value::arr(crate::memory_plane::PLANES.iter().map(|p| Value::s(*p)).collect()))])), ("expected_revision", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1"))]))], vec![Value::s("id"), Value::s("to_plane"), Value::s("expected_revision")]),
        batch_tool("memory_grep", "按行检索正文，返回命中行与行号（search 回答哪条记录，grep 回答哪一行）。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按配置期 --root → 二进制所在目录 → ABOOK_ROOT → ~/AgentsBook 自动推导"))])), ("pattern", obj(vec![("type", Value::s("string")), ("description", Value::s("要匹配的行内文本"))])), ("plane", obj(vec![("type", Value::s("string")), ("description", Value::s("限定平面；省略为全部六个"))])), ("context", obj(vec![("type", Value::s("integer")), ("description", Value::s("命中行前后各带几行，默认 0"))])), ("regex", obj(vec![("type", Value::s("boolean")), ("description", Value::s("true 时把 pattern 当正则"))]))], vec![Value::s("pattern")]),
        batch_tool("memory_trash", "列出软删记录，可当回收站用；配合 memory_restore 恢复。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按配置期 --root → 二进制所在目录 → ABOOK_ROOT → ~/AgentsBook 自动推导"))])), ("plane", obj(vec![("type", Value::s("string")), ("description", Value::s("限定平面；省略为全部六个"))])), ("page", obj(vec![("type", Value::s("integer")), ("description", Value::s("页码，从 1 开始"))])), ("per_page", obj(vec![("type", Value::s("integer")), ("description", Value::s("每页条数"))]))], vec![]),
        batch_tool("memory_history", "读取一条记录的当前快照与相关变更记录，用于回答这条为什么变成现在这样。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按配置期 --root → 二进制所在目录 → ABOOK_ROOT → ~/AgentsBook 自动推导"))])), ("id", obj(vec![("type", Value::s("string")), ("description", Value::s("记录 ID"))])), ("per_page", obj(vec![("type", Value::s("integer")), ("description", Value::s("最多返回多少条，默认 50"))]))], vec![Value::s("id")]),
        batch_tool("memory_merge", "把第二条记录并入第一条并软删第二条；正文并列保留、缺失字段补齐、关系取并集。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按配置期 --root → 二进制所在目录 → ABOOK_ROOT → ~/AgentsBook 自动推导"))])), ("id", obj(vec![("type", Value::s("string")), ("description", Value::s("保留的主记录 ID"))])), ("into", obj(vec![("type", Value::s("string")), ("description", Value::s("被并入后软删的记录 ID"))])), ("title", obj(vec![("type", Value::s("string")), ("description", Value::s("合并后的标题；省略则保留主记录原标题"))])), ("expected_revision", obj(vec![("type", Value::s("integer")), ("description", Value::s("主记录当前 revision，用于防覆盖"))])), ("expected_into_revision", obj(vec![("type", Value::s("integer")), ("description", Value::s("被并入记录当前 revision，用于防覆盖"))]))], vec![Value::s("id"), Value::s("into"), Value::s("expected_revision"), Value::s("expected_into_revision")]),
        batch_tool("memory_split", "按分隔符把一条记录拆成多条，原记录软删并记下派生关系；合并的反向操作。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按配置期 --root → 二进制所在目录 → ABOOK_ROOT → ~/AgentsBook 自动推导"))])), ("id", obj(vec![("type", Value::s("string")), ("description", Value::s("要拆的记录 ID"))])), ("expected_revision", obj(vec![("type", Value::s("integer")), ("description", Value::s("该记录当前 revision，用于防覆盖"))])), ("separator", obj(vec![("type", Value::s("string")), ("description", Value::s("切分分隔符；默认空行加 --- 加空行，即合并时插入的那个分隔线"))])), ("plane", obj(vec![("type", Value::s("string")), ("description", Value::s("拆到哪个平面；省略则沿用原记录的平面"))]))], vec![Value::s("id"), Value::s("expected_revision")]),
        batch_tool("memory_restore", "按 expected_revision 恢复一条普通记忆。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))])), ("id", obj(vec![("type", Value::s("string"))])), ("expected_revision", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("1"))]))], vec![Value::s("id"), Value::s("expected_revision")]),
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
            "memory_list" => &["root", "plane", "limit", "offset", "status", "kind", "include_deleted", "full"][..],
            "memory_search" | "memory_recall" => &["root", "query", "plane", "limit", "per_page", "field", "regex", "full"][..],
            "memory_grep" => &["root", "pattern", "query", "plane", "context", "regex"][..],
            "memory_trash" => &["root", "plane", "page", "per_page", "limit"][..],
            "memory_history" => &["root", "id", "per_page", "limit"][..],
            "memory_merge" => &[
                "root", "id", "into", "title", "expected_revision", "expected_into_revision",
            ][..],
            "memory_split" => &[
                "root", "id", "expected_revision", "separator", "plane",
            ][..],
            "memory_update" => &[
                "root", "id", "expected_revision", "title", "read_when", "content", "body", "fields",
            ][..],
            "memory_delete" | "memory_restore" => &["root", "id", "expected_revision"][..],
            "memory_move_plane" => &["root", "id", "to_plane", "expected_revision"][..],
            _ => return Err(format!("未知 memory 工具: {name}")),
        };
        validate_batch_args(args, allowed)?;
        // root 优先级：显式传参 > 配置期固定的 --root > 运行时推导。
        // 配置期固定排在推导前面，因为写进客户端配置的路径是确定的，
        // 而推导取决于二进制位置、环境变量和 cwd，任一处变动就会指向别的库。
        let root = scalar_text(args, "root")
            .filter(|value| !value.trim().is_empty())
            .or_else(|| attached_root().map(str::to_string))
            .unwrap_or_default();
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
                let filter = crate::memory_plane::ListFilter {
                    status: scalar_text(args, "status"),
                    kind: scalar_text(args, "kind"),
                    include_deleted: matches!(args.get("include_deleted"), Some(Value::Bool(true))),
                };
                crate::memory_plane::list_filtered(&root, &plane, limit, offset, &filter, matches!(args.get("full"), Some(Value::Bool(true))))
                    .map_err(|e| e.msg().to_string())
            }
            "memory_search" | "memory_recall" => {
                let query = scalar_text(args, "query").ok_or("memory_search 缺少 query")?;
                let plane = scalar_text(args, "plane");
                let limit = scalar_text(args, "limit")
                    .or_else(|| scalar_text(args, "per_page"))
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(20);
                let fields = match scalar_text(args, "field").as_deref() {
                    Some(v) if v.eq_ignore_ascii_case("title") => crate::memory_plane::SearchFields::Title,
                    Some(v) if v.eq_ignore_ascii_case("content") || v.eq_ignore_ascii_case("body") => crate::memory_plane::SearchFields::Content,
                    Some(v) if v.eq_ignore_ascii_case("read-when") || v.eq_ignore_ascii_case("read_when") => crate::memory_plane::SearchFields::ReadWhen,
                    _ => crate::memory_plane::SearchFields::All,
                };
                let regex = matches!(args.get("regex"), Some(Value::Bool(true)));
                crate::memory_plane::search_filtered(&root, &query, plane.as_deref(), limit, fields, regex, matches!(args.get("full"), Some(Value::Bool(true))))
                    .map_err(|e| e.msg().to_string())
            }
            "memory_grep" => {
                let pattern = scalar_text(args, "pattern")
                    .or_else(|| scalar_text(args, "query"))
                    .ok_or("memory_grep 缺少 pattern")?;
                let plane = scalar_text(args, "plane");
                let context = scalar_text(args, "context").and_then(|s| s.parse().ok()).unwrap_or(0);
                let regex = matches!(args.get("regex"), Some(Value::Bool(true)));
                crate::memory_plane::grep(&root, &pattern, plane.as_deref(), context, regex)
                    .map_err(|e| e.msg().to_string())
            }
            "memory_trash" => {
                let plane = scalar_text(args, "plane");
                let limit = scalar_text(args, "per_page")
                    .or_else(|| scalar_text(args, "limit"))
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(20);
                let page = scalar_text(args, "page")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(1)
                    .max(1);
                crate::memory_plane::trash(&root, plane.as_deref(), limit, (page - 1) * limit)
                    .map_err(|e| e.msg().to_string())
            }
            "memory_history" => {
                let id = scalar_text(args, "id").ok_or("memory_history 缺少 id")?;
                let limit = scalar_text(args, "per_page")
                    .or_else(|| scalar_text(args, "limit"))
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(50);
                crate::memory_plane::history(&root, &id, limit).map_err(|e| e.msg().to_string())
            }
            "memory_split" => {
                let id = scalar_text(args, "id").ok_or("memory_split 缺少 id")?;
                let expected = scalar_text(args, "expected_revision")
                    .ok_or("memory_split 缺少 expected_revision")?;
                let separator = scalar_text(args, "separator");
                let plane = scalar_text(args, "plane");
                crate::memory_plane::split(
                    &root,
                    &id,
                    &expected,
                    separator.as_deref(),
                    plane.as_deref(),
                )
                .map_err(|e| e.msg().to_string())
            }
            "memory_merge" => {
                let id = scalar_text(args, "id").ok_or("memory_merge 缺少 id")?;
                let into = scalar_text(args, "into").ok_or("memory_merge 缺少 into")?;
                let primary = scalar_text(args, "expected_revision")
                    .ok_or("memory_merge 缺少 expected_revision")?;
                let secondary = scalar_text(args, "expected_into_revision")
                    .ok_or("memory_merge 缺少 expected_into_revision")?;
                crate::memory_plane::merge(
                    &root,
                    &id,
                    &into,
                    scalar_text(args, "title").as_deref(),
                    &primary,
                    &secondary,
                )
                .map_err(|e| e.msg().to_string())
            }
            "memory_update" => {
                let id = scalar_text(args, "id").ok_or("memory_update 缺少 id")?;
                let revision = scalar_text(args, "expected_revision").ok_or("memory_update 缺少 expected_revision")?;
                let content = scalar_text(args, "content").or_else(|| scalar_text(args, "body"));
                let fields = object_fields(args, "fields")?;
                crate::memory_plane::update(&root, &id, &revision, scalar_text(args, "title").as_deref(), scalar_text(args, "read_when").as_deref(), content.as_deref(), &fields).map_err(|e| e.msg().to_string())
            }
            "memory_move_plane" => {
                let id = scalar_text(args, "id").ok_or("memory_move_plane 缺少 id")?;
                let to_plane = scalar_text(args, "to_plane").ok_or("memory_move_plane 缺少 to_plane")?;
                let revision = scalar_text(args, "expected_revision").ok_or("memory_move_plane 缺少 expected_revision")?;
                crate::memory_plane::move_plane(&root, &id, &to_plane, &revision).map_err(|e| e.msg().to_string())
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
        Ok(data) => complete(tool_ok(data)),
        Err(message) => complete(obj(vec![("content", Value::arr(vec![obj(vec![("type", Value::s("text")), ("text", Value::s(message))])])), ("isError", Value::b(true))])),
    }
}

fn batch_tools() -> Vec<Value> {
    vec![
        batch_tool("memory_batch_preview", "预览一批改动并写入 durable preview journal，不修改目标文件。两种用法二选一：给 records 按记录改（推荐），或给 path+body 按文件改。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按配置期 --root → 二进制所在目录 → ABOOK_ROOT → ~/AgentsBook 自动推导"))])), ("records", obj(vec![("type", Value::s("array")), ("description", Value::s("按记录批量改：每项含 id、expected_revision，以及要改的 title/read_when/content/status/fields。给这个就不用给 path+body")), ("items", obj(vec![("type", Value::s("object"))]))])), ("path", obj(vec![("type", Value::s("string")), ("description", Value::s("按文件改时用：相对库根的路径"))])), ("body", obj(vec![("type", Value::s("string")), ("description", Value::s("按文件改时用：整篇文本"))]))], vec![]),
        batch_tool("memory_batch_apply", "按 preview token 和 expected revision 原子应用暂存变更。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))])), ("token", obj(vec![("type", Value::s("string"))])), ("expected_revision", obj(vec![("type", Value::s("integer")), ("minimum", Value::n("0"))]))], vec![Value::s("token"), Value::s("expected_revision")]),
        batch_tool("memory_batch_rollback", "回滚已提交事务并推进新的库 revision。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))])), ("token", obj(vec![("type", Value::s("string"))]))], vec![Value::s("token")]),
        batch_tool("memory_batch_status", "读取未完成事务的恢复状态，不改目标文件。", vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))]))], vec![]),
    ]
}

/// 把 `fields` 对象展开成键值对，供 update 写头部字段。
///
/// 头部字段是开放集合（KIND/MATURITY/AUTHORITY/KEYS/NOT_FOR/ENTITIES…），
/// 给每个开一个独立参数会让参数面随字段增长而膨胀，所以用一个对象承载。
/// 值统一按字符串写入：这些字段本来就是文本语义，数组会按逗号拼接——
/// 与旧记录格式一致，读回时也能被 legacy_value 正确解析成 relations。
fn object_fields(args: &Value, key: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    match args.get(key) {
        None | Some(Value::Null) => return Ok(out),
        Some(Value::Object(map)) => {
            for (name, value) in map {
                let text = match value {
                    Value::String(s) | Value::Number(s) => s.clone(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => String::new(),
                    Value::Array(items) => items
                        .iter()
                        .map(|item| match item {
                            Value::String(s) | Value::Number(s) => s.clone(),
                            other => mcp_json::stringify(other),
                        })
                        .collect::<Vec<_>>()
                        .join(","),
                    other => mcp_json::stringify(other),
                };
                out.push((name.clone(), text));
            }
        }
        Some(_) => return Err(format!("{key} 必须是对象，形如 {{\"KIND\":\"DECISION\"}}")),
    }
    Ok(out)
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
            "memory_batch_preview" => &["root", "path", "body", "records"][..],
            "memory_batch_apply" => &["root", "token", "expected_revision"][..],
            "memory_batch_rollback" => &["root", "token"][..],
            "memory_batch_status" => &["root"][..],
            _ => return Err(format!("未知批量工具: {name}")),
        };
        validate_batch_args(args, allowed)?;
        let root = scalar_text(args, "root")
            .filter(|value| !value.trim().is_empty())
            .or_else(|| attached_root().map(str::to_string))
            .unwrap_or_default();
        match name {
            "memory_batch_preview" => {
                // 两种用法二选一：records 按记录改（推荐），path+body 按文件改。
                //
                // 事务层本身只有文件粒度，但六平面是记录模型——调用方要批量改十条
                // 记录，不该被迫先查出存储文件名再拼整篇记录文本。records 这一层
                // 就是把"按 ID 改"翻译成"按文件改"，落盘仍走同一套原子提交。
                let records = args.get("records").cloned();
                let path = scalar_text(args, "path");
                let body = scalar_text(args, "body");
                match (records, path, body) {
                    (Some(Value::Array(items)), None, None) => {
                        if items.is_empty() { return Err("records 不能是空数组".to_string()); }
                        let planned = crate::memory_plane::plan_record_updates(&root, &items)
                            .map_err(|e| e.msg().to_string())?;
                        let mut tx = crate::txn::Transaction::prepare(&root).map_err(|e| e.msg().to_string())?;
                        let mut changes = Vec::new();
                        for change in &planned {
                            tx.stage_file(crate::txn::Selector::new(change.path.clone()), &change.body)
                                .map_err(|e| e.msg().to_string())?;
                            changes.push(obj(vec![
                                ("id", Value::s(change.id.clone())),
                                ("path", Value::s(change.path.clone())),
                                ("revision", Value::s(change.revision.clone())),
                            ]));
                        }
                        Ok(obj(vec![
                            ("token", Value::s(tx.token().to_string())),
                            ("revision", Value::n(tx.revision.to_string())),
                            ("mode", Value::s("records")),
                            ("count", Value::n(changes.len().to_string())),
                            ("changes", Value::Array(changes)),
                        ]))
                    }
                    (None, Some(path), Some(body)) => {
                        // 落在 `planes/<平面>/records/` 下的内容必须是可解析的记录。
                        // 不校验的后果不是"多一个文件"，而是整库读不了：records() 是
                        // list/context/search/export 的共同入口，一条坏文件让它报错退出。
                        crate::memory_plane::validate_record_path_and_body(std::path::Path::new(&root), &path, &body)
                            .map_err(|e| e.msg().to_string())?;
                        let mut tx = crate::txn::Transaction::prepare(&root).map_err(|e| e.msg().to_string())?;
                        tx.stage_file(crate::txn::Selector::new(path), &body).map_err(|e| e.msg().to_string())?;
                        Ok(obj(vec![
                            ("token", Value::s(tx.token().to_string())),
                            ("revision", Value::n(tx.revision.to_string())),
                            ("mode", Value::s("file")),
                        ]))
                    }
                    (Some(_), _, _) => Err("records 必须是对象数组；用 records 时不要再给 path/body".to_string()),
                    _ => Err("preview 需要 records 数组，或同时给 path 和 body".to_string()),
                }
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
        Ok(data) => complete(tool_ok(data)),
        Err(message) => complete(obj(vec![("content", Value::arr(vec![obj(vec![("type", Value::s("text")), ("text", Value::s(message))])])), ("isError", Value::b(true))])),
    }
}

fn helper_tool(name: &str, description: &str) -> Value {
    let (properties, required): (Vec<(&str, Value)>, Vec<Value>) = match name {
        "spec_get" => (vec![("name", obj(vec![("type", Value::s("string"))]))], vec![Value::s("name")]),
        "skill_list" => (vec![("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))]))], vec![]),
        "skill_read" => (vec![("name", obj(vec![("type", Value::s("string"))])), ("root", obj(vec![("type", Value::s("string")), ("description", Value::s("可选：记忆库根。省略时按二进制所在目录→ABOOK_ROOT→~/AgentsBook 自动推导"))]))], vec![Value::s("name")]),
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

/// 直接经内存内核处理（不走 CLI 子进程）的 memory 工具。
///
/// 唯一真源：新增 memory_* 工具时只改这里，schema 与分发自动一致。
fn is_direct_memory_tool(name: &str) -> bool {
    memory_tools()
        .iter()
        .filter_map(|tool| tool.get("name").and_then(as_string))
        .any(|declared| declared == name)
        && !matches!(
            name,
            "memory_batch_preview" | "memory_batch_apply" | "memory_batch_rollback" | "memory_batch_status"
        )
}

fn tools_call(req: &Value, _legacy: bool) -> Value {
    let params = req.get("params");
    let name = params.and_then(|p| p.str("name")).unwrap_or("");
    if !tool_is_registered(name) {
        return error_value(-32602, &format!("未知工具: {name}"), None);
    }
    let args = params.and_then(|p| p.get("arguments"));
    // 这份清单必须与 memory_tools() 的 schema 一致。原先它被手抄了两份，
    // 加新工具时漏一处就出现"schema 说有、调用报未知工具"——最难查的不一致。
    // 现在统一由 is_direct_memory_tool 判定，只有一处定义。
    if is_direct_memory_tool(name) {
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
    let attachment_json = mcp_json::parse(&out).ok();
    let empty_attachment_list = name == "abook_attach_list"
        && code == 3
        && matches!(attachment_json.as_ref().and_then(|v| v.get("count")), Some(Value::Number(n)) if n == "0")
        && matches!(attachment_json.as_ref().and_then(|v| v.get("items")), Some(Value::Array(items)) if items.is_empty());
    let payload = mcp_json::parse(&out).unwrap_or_else(|_| {
        obj(vec![("stdout", Value::s(out)), ("stderr", Value::s(err)), ("exit", Value::n(code.to_string()))])
    });
    complete(tool_result(payload, code != 0 && !empty_attachment_list))
}

fn legacy_readonly_command(name: &str) -> bool {
    !matches!(name, "write" | "add" | "promote" | "update" | "edit" | "delete" | "rm" | "restore" | "set" | "unset" | "append" | "prepend" | "patch" | "rename" | "move" | "mv" | "copy" | "cp" | "demote" | "merge" | "split" | "repair" | "fix" | "init" | "refresh" | "sync" | "normalize" | "commit" | "branch" | "recount" | "leftover" | "vault" | "install" | "self-install" | "upgrade" | "self-update")
}

fn tool_is_registered(name: &str) -> bool {
    if crate::mcp_registry::public_tools().iter().any(|tool| tool.name == name && tool.is_public()) { return true; }
    // memory 工具不再手抄名单：只要写进 memory_tools()/batch_tools() 就自动被认可。
    // 这份名单手抄过两次，两次都漏了新工具，症状是"tools/list 里有、调用报未知工具"。
    // 由 schema 驱动之后，这类漏登记在结构上不可能再发生。
    // 先绑定到局部变量：`Vec` 的临时值在 `.iter()` 借用期间会被立刻释放，
    // 直接把两次调用串起来写会得到一个借用已释放值的编译错误。
    let declared = memory_tools();
    let batched = batch_tools();
    if declared.iter().chain(batched.iter()).any(|tool| tool.get("name").and_then(as_string) == Some(name)) {
        return true;
    }
    matches!(name, "memory_help" | "spec_list" | "spec_get" | "skill_list" | "skill_read" | "system_update")
}

/// 自检：schema 里声明的每个 memory_* 工具都必须能被白名单接受，反之亦然。
///
/// 这条来自一个真实事故：`memory_move_plane` 已写进 tools/list 的 schema、
/// 也已实现分发，却漏进 `tool_is_registered` 的白名单，于是调用直接得到
/// JSON-RPC 层错误而不是工具结果。schema 说有、实际调不通，是最难查的一类
/// 不一致——调用方看到工具存在，却永远失败。
#[cfg(test)]
pub fn assert_memory_tools_consistent() -> Vec<String> {
    let declared: Vec<String> = memory_tools()
        .iter()
        .filter_map(|tool| tool.get("name").and_then(as_string).map(str::to_string))
        .collect();
    let mut problems = Vec::new();
    for name in &declared {
        if !tool_is_registered(name) {
            problems.push(format!("schema 已声明但白名单拒绝: {name}"));
        }
    }
    // 第二道：白名单通过还不够，必须真的进到内存内核。
    // 原先 tools_call 里另有一份手抄清单，漏项时工具会掉到 CLI 路由并报"未知工具"。
    for name in &declared {
        let is_batch = matches!(
            name.as_str(),
            "memory_batch_preview" | "memory_batch_apply" | "memory_batch_rollback" | "memory_batch_status"
        );
        if !is_batch && !is_direct_memory_tool(name) {
            problems.push(format!("schema 已声明但未进内存内核（会掉到 CLI 路由）: {name}"));
        }
    }
    for name in &declared {
        let probe = obj(vec![("root", Value::s("/nonexistent-probe"))]);
        if matches!(direct_memory_call(name, &probe), Value::Null) {
            problems.push(format!("分发返回空: {name}"));
        }
    }
    problems
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
        // 配置期固定的库根要落到每个走 CLI 子进程的工具上。
        // `build_argv` 只认参数里的 root，如果这里不补，所有 `abook_*` 工具
        // （附件、vault、spec、skill…）在"配置期 --root"模式下都会去猜库根，
        // 猜错就报"没有找到附件记录"这种看着像数据问题的错误。
        let mut effective = a.clone();
        if effective.str("root").map(|v| v.trim().is_empty()).unwrap_or(true) {
            if let Some(attached) = attached_root() {
                if let Value::Object(ref mut map) = effective {
                    map.insert("root".into(), Value::s(attached.to_string()));
                }
            }
        }
        return match crate::mcp_registry::build_argv(name, &effective) {
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
/// 工具成功返回：**只给一份数据**。
///
/// 以前每个结果同时给 `content.text` 与 `structuredContent`，两者是同一份输出的
/// 两个完整副本——一次 memory_search 三万字，其中一半纯属重复；列表类更糟，每条
/// 记录的完整正文都出现两次。MCP 客户端读哪个都行，给一份就够。
fn tool_ok(data: Value) -> Value { tool_result(data, false) }

fn tool_result(payload: Value, is_error: bool) -> Value {
    obj(vec![
        ("content", Value::arr(vec![])),
        ("structuredContent", payload),
        ("isError", Value::b(is_error)),
    ])
}

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

#[cfg(test)]
mod consistency_tests {
    #[test]
    fn memory_tools_all_reachable() {
        let problems = super::assert_memory_tools_consistent();
        assert!(problems.is_empty(), "工具面不一致: {problems:?}");
    }
}
