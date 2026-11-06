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

pub fn cmd_memory(a: &Args) -> R<i32> {
    let sub = a.pos_or(0, "context");
    let root = root(a);
    match sub.as_str() {
        "context" => emit(memory_plane::context(&root)?),
        "export" => emit(memory_plane::export(&root, a.get("plane"))?),
        "write" | "add" => {
            let plane = required(a, "plane")?;
            let title = required(a, "title")?;
            let content = a.get("content").or_else(|| a.get("body")).ok_or_else(|| usage("memory write 缺少 --content"))?;
            emit(memory_plane::write(&root, plane, title, a.get("read-when").unwrap_or(""), content)?)
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
        _ => usage(format!("未知 memory 子命令 `{sub}`；可用 context/export/write/read/list/search/update/delete/restore")),
    }
}
