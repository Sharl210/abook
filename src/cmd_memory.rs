//! Plane-aware CLI entry point backed by the same structured runtime as MCP.
use crate::memory_plane;
use crate::mcp_json;
use crate::util::*;

fn emit(value: mcp_json::Value) -> R<i32> {
    crate::say_line!("{}", mcp_json::stringify(&value));
    Ok(0)
}

fn root(a: &Args) -> String {
    resolve_root(a).to_string_lossy().to_string()
}

fn required<'a>(a: &'a Args, key: &str) -> R<&'a str> { a.req(key) }

fn body(a: &Args) -> R<String> {
    if let Some(path) = a.get("body-file") {
        if path == "-" {
            let mut value = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut value)?;
            return Ok(value);
        }
        return read_str(&expand_tilde(path));
    }
    if let Some(content) = a.get("content").or_else(|| a.get("body")) {
        return Ok(content.to_string());
    }
    if a.has("stdin") {
        let mut value = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut value)?;
        return Ok(value);
    }
    Err(match usage::<()>("memory write 缺少 --content / --body-file / --stdin") { Err(e) => e, Ok(()) => unreachable!() })
}

fn page_args(a: &Args) -> (usize, usize) {
    let limit = a.num("limit").or_else(|| a.num("per-page")).unwrap_or(100).max(1) as usize;
    let page = a.num("page").unwrap_or(1).max(1) as usize;
    let offset = a.num("offset").map(|v| v.max(0) as usize).unwrap_or_else(|| page.saturating_sub(1).saturating_mul(limit));
    (limit, offset)
}

fn field_value(value: &mcp_json::Value, field: &str) -> Option<mcp_json::Value> {
    let key = match field.to_ascii_uppercase().as_str() {
        "ID" => "id", "TITLE" => "title", "BODY" | "MEMORY_BODY" => "content",
        "READ_WHEN" => "read_when", "STATUS" => "status", "REVISION" => "revision",
        "PLANE" => "plane", other => return value.get(other.to_ascii_lowercase().as_str()).cloned(),
    };
    value.get(key).cloned()
}

pub fn cmd_memory(a: &Args) -> R<i32> {
    let sub = a.pos_or(0, "context");
    let root = root(a);
    match sub.as_str() {
        "init" => {
            memory_plane::initialize_layout(&root)?;
            emit(mcp_json::Value::obj([("layout", mcp_json::Value::s(memory_plane::LAYOUT_MARKER))].into_iter().map(|(k,v)| (k.to_string(),v)).collect()))
        }
        "index" => { let (limit, offset) = page_args(a); emit(memory_plane::index(&root, a.get("plane"), limit, offset)?) },
        "context" => emit(memory_plane::context(&root)?),
        "paths" => emit(memory_plane::paths(&root)?),
        "export" => emit(memory_plane::export(&root, a.get("plane"))?),
        "write" | "add" => {
            let plane = required(a, "plane")?;
            let title = required(a, "title")?;
            let content = body(a)?;
            emit(memory_plane::write(&root, plane, title, a.get("read-when").unwrap_or(""), &content)?)
        }
        "read" | "show" => {
            let value = memory_plane::read(&root, required(a, "id")?)?;
            match a.get("field") {
                Some(field) => emit(field_value(&value, field).ok_or_else(|| match usage::<()>(format!("memory get 没有字段: {field}")) { Err(e) => e, Ok(()) => unreachable!() })?),
                None => emit(value),
            }
        }
        "list" | "ls" => {
            let plane = a.get("plane").unwrap_or("all");
            let (limit, offset) = page_args(a);
            let filter = memory_plane::ListFilter {
                status: a.get("status").map(str::to_string),
                kind: a.get("kind").map(str::to_string),
                include_deleted: a.has("include-deleted") || a.has("deleted"),
            };
            emit(memory_plane::list_filtered(&root, plane, limit, offset, &filter)?)
        }
        "search" | "recall" => {
            let fields = match a.get("field").or_else(|| a.get("fields")) {
                Some(name) if name.eq_ignore_ascii_case("title") => memory_plane::SearchFields::Title,
                Some(name) if name.eq_ignore_ascii_case("content") || name.eq_ignore_ascii_case("body") => memory_plane::SearchFields::Content,
                Some(name) if name.eq_ignore_ascii_case("read-when") || name.eq_ignore_ascii_case("read_when") => memory_plane::SearchFields::ReadWhen,
                Some(name) if name.eq_ignore_ascii_case("all") => memory_plane::SearchFields::All,
                Some(name) => return usage(format!("未知检索字段 `{name}`；可用 all/title/content/read-when")),
                None => memory_plane::SearchFields::All,
            };
            emit(memory_plane::search_filtered(
                &root,
                required(a, "query")?,
                a.get("plane"),
                a.num("per-page").unwrap_or(20) as usize,
                fields,
                a.has("regex"),
            )?)
        }
        "grep" => emit(memory_plane::grep(
            &root,
            required(a, "pattern").or_else(|_| required(a, "query"))?,
            a.get("plane"),
            a.num("context").unwrap_or(0) as usize,
            a.has("regex"),
        )?),
        "trash" | "deleted" => emit(memory_plane::trash(
            &root,
            a.get("plane"),
            page_args(a).0,
            page_args(a).1,
        )?),
        "history" | "log" => emit(memory_plane::history(
            &root,
            required(a, "id")?,
            a.num("per-page").unwrap_or(50) as usize,
        )?),
        "merge" => emit(memory_plane::merge(
            &root,
            required(a, "id")?,
            required(a, "into")?,
            a.get("title"),
            required(a, "expect-revision")?,
            required(a, "expect-into-revision")?,
        )?),
        "split" => emit(memory_plane::split(
            &root,
            required(a, "id")?,
            required(a, "expect-revision")?,
            a.get("separator").as_deref(),
            a.get("to-plane").or_else(|| a.get("plane")).as_deref(),
        )?),
        "update" | "edit" => {
            let content = if a.get("body-file").is_some() || a.has("stdin") {
                Some(body(a)?)
            } else {
                a.get("content").or_else(|| a.get("body")).map(str::to_string)
            };
            emit(memory_plane::update(&root, required(a, "id")?, required(a, "expect-revision")?, a.get("title"), a.get("read-when"), content.as_deref(), &parse_fields(a)?)?)
        }
        "delete" | "rm" => emit(memory_plane::delete(&root, required(a, "id")?, required(a, "expect-revision")?)?),
        "restore" => emit(memory_plane::restore(&root, required(a, "id")?, required(a, "expect-revision")?)?),
        "move-plane" | "move" => emit(memory_plane::move_plane(&root, required(a, "id")?, required(a, "to-plane")?, required(a, "expect-revision")?)?),
        _ => usage(format!("未知 memory 子命令 `{sub}`；可用 init/index/context/paths/export/write/read/list/search/grep/trash/history/merge/update/delete/restore/move-plane")),
    }
}

/// 解析 `--field KEY=VALUE`，可重复。
///
/// 头部字段（KIND/MATURITY/AUTHORITY/KEYS/NOT_FOR/ENTITIES 等）此前没有任何
/// 写入路径，想改只能删掉整条重写——那会断掉 ID、关系链接和附件清单。
/// 用可重复的 `--field` 而不是给每个字段开一个专用 flag：头部字段是开放集合，
/// 每加一个就改一次参数面不现实。
fn parse_fields(a: &Args) -> R<Vec<(String, String)>> {
    let mut out = Vec::new();
    for raw in a.kv.get("field").map(|v| v.as_slice()).unwrap_or(&[]) {
        let (key, value) = match raw.split_once('=') {
            Some((k, v)) => (k.trim(), v),
            None => return usage(format!("--field 需要 KEY=VALUE 形式，收到: {raw}")),
        };
        if key.is_empty() {
            return usage(format!("--field 的键不能为空: {raw}"));
        }
        out.push((key.to_string(), value.to_string()));
    }
    Ok(out)
}
