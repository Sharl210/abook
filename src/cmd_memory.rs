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
    if a.has("stdin") {
        let mut value = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut value)?;
        return Ok(value);
    }
    a.get("content").or_else(|| a.get("body")).map(str::to_string).ok_or_else(|| match usage::<()>("memory write 缺少 --content / --body-file / --stdin") { Err(e) => e, Ok(()) => unreachable!() })
}

pub fn cmd_memory(a: &Args) -> R<i32> {
    let sub = a.pos_or(0, "context");
    let root = root(a);
    match sub.as_str() {
        "init" => {
            memory_plane::initialize_layout(&root)?;
            emit(mcp_json::Value::obj([("layout", mcp_json::Value::s(memory_plane::LAYOUT_MARKER))].into_iter().map(|(k,v)| (k.to_string(),v)).collect()))
        }
        "index" => emit(memory_plane::index(&root, a.get("plane"), a.num("limit").or_else(|| a.num("per-page")).unwrap_or(100) as usize, a.num("offset").or_else(|| a.num("page")).unwrap_or(0) as usize)?),
        "context" => emit(memory_plane::context(&root)?),
        "export" => emit(memory_plane::export(&root, a.get("plane"))?),
        "write" | "add" => {
            let plane = required(a, "plane")?;
            let title = required(a, "title")?;
            let content = body(a)?;
            emit(memory_plane::write(&root, plane, title, a.get("read-when").unwrap_or(""), &content)?)
        }
        "read" | "show" => emit(memory_plane::read(&root, required(a, "id")?)?),
        "list" | "ls" => {
            let plane = required(a, "plane")?;
            emit(memory_plane::list(&root, plane, a.num("per-page").unwrap_or(100) as usize, a.num("page").unwrap_or(0) as usize)?)
        }
        "search" | "recall" => emit(memory_plane::search(&root, required(a, "query")?, a.get("plane"), a.num("per-page").unwrap_or(20) as usize)?),
        "update" | "edit" => {
            let content = a.get("content").or_else(|| a.get("body"));
            emit(memory_plane::update(&root, required(a, "id")?, required(a, "expect-revision")?, a.get("title"), a.get("read-when"), content)?)
        }
        "delete" | "rm" => emit(memory_plane::delete(&root, required(a, "id")?, required(a, "expect-revision")?)?),
        "restore" => emit(memory_plane::restore(&root, required(a, "id")?, required(a, "expect-revision")?)?),
        _ => usage(format!("未知 memory 子命令 `{sub}`；可用 index/context/export/write/read/list/search/update/delete/restore")),
    }
}
