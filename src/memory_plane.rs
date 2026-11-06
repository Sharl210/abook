//! 六个普通记忆平面的最小结构化运行时内核；credentials 仍由 vault 独立管理。
use crate::mcp_json::{self, Value};
use crate::util::{write_atomic, EE, LibLock, R};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PLANES: &[&str] = &["soul", "rules", "profile", "explicit", "long_term", "episodic"];

fn err(message: impl Into<String>) -> EE { match crate::util::integrity::<()>(message) { Err(e) => e, Ok(()) => unreachable!() } }
fn plane_ok(plane: &str) -> bool { PLANES.contains(&plane) }
fn root_path(root: &str) -> R<PathBuf> {
    if root.is_empty() { return Err(err("memory 工具需要 root")); }
    let args = crate::util::Args { cmd: String::new(), pos: Vec::new(), kv: BTreeMap::from([("root".to_string(), vec![root.to_string()])]) };
    let resolved = crate::util::resolve_root(&args);
    if resolved.exists() {
        let meta = fs::symlink_metadata(&resolved)?;
        if meta.file_type().is_symlink() || !meta.is_dir() { return Err(err("memory root 必须是普通目录")); }
        Ok(fs::canonicalize(resolved)?)
    } else { Ok(resolved) }
}
fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }
fn text(value: &Value, key: &str) -> Option<String> { value.get(key).and_then(|v| match v { Value::String(s) => Some(s.clone()), Value::Number(n) => Some(n.clone()), _ => None }) }
fn object(mut values: Vec<(&str, Value)>) -> Value { let mut map = BTreeMap::new(); for (k, v) in values.drain(..) { map.insert(k.to_string(), v); } Value::Object(map) }
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 160 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn ensure_plane_dirs(root: &Path, plane: &str) -> R<PathBuf> {
    let p = root.join("planes").join(plane);
    let records = p.join("records");
    for dir in [root.join("planes"), p, records.clone()] {
        if dir.exists() {
            let meta = fs::symlink_metadata(&dir)?;
            if meta.file_type().is_symlink() || !meta.is_dir() { return Err(err(format!("记忆目录必须是普通目录: {}", dir.display()))); }
        }
    }
    Ok(records)
}

fn file_for(root: &Path, plane: &str, id: &str) -> R<PathBuf> {
    if !plane_ok(plane) || !valid_id(id) { return Err(err("非法 plane 或 record id")); }
    Ok(ensure_plane_dirs(root, plane)?.join(format!("{id}.json")))
}
fn records(root: &Path, plane: Option<&str>) -> R<Vec<Value>> {
    let planes: Vec<&str> = match plane { Some(p) => vec![p], None => PLANES.to_vec() };
    let mut out = Vec::new();
    for p in planes {
        if !plane_ok(p) { return Err(err(format!("未知记忆平面: {p}"))); }
        let dir = ensure_plane_dirs(root, p)?;
        if !dir.is_dir() { continue; }
        let mut files: Vec<_> = fs::read_dir(&dir)?.collect::<Result<Vec<_>, _>>()?;
        files.sort_by_key(|e| e.file_name());
        for entry in files {
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() { return Err(err(format!("记忆目录含符号链接: {}", path.display()))); }
            if !meta.is_file() || path.extension().and_then(|s| s.to_str()) != Some("json") { continue; }
            let raw = fs::read_to_string(&path)?;
            let value = mcp_json::parse(&raw).map_err(|e| err(format!("解析记忆记录失败 {}: {e}", path.display())))?;
            if text(&value, "status").as_deref() != Some("deleted") { out.push(value); }
        }
    }
    Ok(out)
}

pub fn write(root: &str, plane: &str, title: &str, read_when: &str, content: &str) -> R<Value> {
    if !plane_ok(plane) { return Err(err(format!("未知记忆平面: {plane}"))); }
    if title.trim().is_empty() || content.is_empty() { return Err(err("memory_write 需要非空 title 和 content")); }
    let root = root_path(root)?;
    let _lock = LibLock::acquire(&root, 30)?;
    let stamp = now();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let seed = format!("{plane}\0{title}\0{content}\0{nanos}\0{}", std::process::id());
    let id = format!("M-{stamp}-{}", crate::sha256::digest_hex(seed.as_bytes()));
    let path = file_for(&root, plane, &id)?;
    if path.exists() { return Err(err("memory record ID collision，拒绝覆盖已有记录")); }
    let value = object(vec![
        ("id", Value::s(id.clone())), ("plane", Value::s(plane)), ("title", Value::s(title)),
        ("content", Value::s(content)), ("read_when", Value::s(read_when)), ("tags", Value::arr(Vec::new())),
        ("entities", Value::arr(Vec::new())), ("source", Value::s("memory_api")), ("confidence", Value::s("observed")),
        ("created_at", Value::n(stamp.to_string())), ("updated_at", Value::n(stamp.to_string())),
        ("relations", Value::arr(Vec::new())), ("legacy_ids", Value::arr(Vec::new())), ("revision", Value::n("1")), ("status", Value::s("active")),
    ]);
    write_atomic(&path, &mcp_json::stringify(&value))?;
    let actual = fs::read_to_string(&path)?;
    let parsed = mcp_json::parse(&actual).map_err(|e| err(e))?;
    if parsed != value { return Err(err("memory record 写入后读回不一致")); }
    Ok(value)
}

pub fn read(root: &str, id: &str) -> R<Value> {
    let root = root_path(root)?;
    let mut found = Vec::new();
    for plane in PLANES {
        let path = record_path(&root, plane, id)?;
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => return Err(err("记忆记录必须是普通文件")),
            Ok(_) => found.push(mcp_json::parse(&fs::read_to_string(path)?).map_err(|e| err(format!("记忆记录 JSON 无法解析: {e}")))?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.into()),
        }
    }
    match found.len() { 1 => Ok(found.remove(0)), 0 => crate::util::notfound::<Value>(format!("没有找到记忆记录: {id}")), _ => Err(err(format!("记忆 ID 跨平面歧义: {id}"))) }
}

pub fn search(root: &str, query: &str, plane: Option<&str>, limit: usize) -> R<Value> {
    if query.trim().is_empty() { return Err(err("memory_search 需要非空 query")); }
    let root = root_path(root)?;
    let needle = query.to_lowercase();
    let mut hits = Vec::new();
    for item in records(&root, plane)? {
        let hay = [text(&item, "title"), text(&item, "content"), text(&item, "read_when")].into_iter().flatten().collect::<Vec<_>>().join("\n").to_lowercase();
        if needle.is_empty() || hay.contains(&needle) { hits.push(item); if hits.len() >= limit.max(1) { break; } }
    }
    Ok(Value::Array(hits))
}

pub fn list(root: &str, plane: &str, limit: usize, offset: usize) -> R<Value> {
    if !plane_ok(plane) { return Err(err(format!("未知记忆平面: {plane}"))); }
    let root = root_path(root)?;
    let mut items = records(&root, Some(plane))?;
    items.sort_by_key(|v| text(v, "id").unwrap_or_default());
    let total = items.len();
    let start = offset.min(total);
    let size = limit.clamp(1, 500);
    let page = items.into_iter().skip(start).take(size).collect();
    Ok(object(vec![("items", Value::Array(page)), ("total", Value::n(total.to_string())), ("offset", Value::n(start.to_string())), ("limit", Value::n(size.to_string()))]))
}
pub fn context(root: &str) -> R<Value> {
    let root = root_path(root)?;
    let mut counts = BTreeMap::new();
    for plane in PLANES { counts.insert((*plane).to_string(), Value::n(records(&root, Some(plane))?.len().to_string())); }
    Ok(object(vec![("planes", Value::Object(counts)), ("credentials", Value::s("vault")), ("total", Value::n(records(&root, None)?.len().to_string()))]))
}

fn record_path(root: &Path, plane: &str, id: &str) -> R<PathBuf> {
    let path = file_for(root, plane, id)?;
    if path.exists() {
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() || !meta.is_file() { return Err(err("记忆记录必须是普通文件")); }
    }
    Ok(path)
}

fn value_field(value: &Value, key: &str) -> Option<Value> { value.get(key).cloned() }
fn version_of(value: &Value) -> String { text(value, "revision").unwrap_or_else(|| "1".to_string()) }

pub fn update(root: &str, id: &str, expected_revision: &str, title: Option<&str>, read_when: Option<&str>, content: Option<&str>) -> R<Value> {
    let root = root_path(root)?; let _lock = LibLock::acquire(&root, 30)?;
    let current = read(root.to_string_lossy().as_ref(), id)?;
    if version_of(&current) != expected_revision { return Err(err(format!("记忆 revision 冲突: expected={expected_revision} actual={}", version_of(&current)))); }
    let plane = text(&current, "plane").ok_or_else(|| err("记忆记录缺少 plane"))?;
    let mut map = match current.clone() { Value::Object(m) => m, _ => return Err(err("记忆记录必须是对象")) };
    if let Some(v) = title { map.insert("title".into(), Value::s(v)); }
    if let Some(v) = read_when { map.insert("read_when".into(), Value::s(v)); }
    if let Some(v) = content { map.insert("content".into(), Value::s(v)); }
    let next = version_of(&current).parse::<u64>().unwrap_or(1).saturating_add(1);
    map.insert("revision".into(), Value::n(next.to_string())); map.insert("updated_at".into(), Value::n(now().to_string()));
    let next = Value::Object(map); let path = record_path(&root, &plane, id)?; let serialized = mcp_json::stringify(&next); crate::util::write_atomic_cas(&path, &serialized, &mcp_json::stringify(&current))?;
    Ok(next)
}

pub fn delete(root: &str, id: &str, expected_revision: &str) -> R<Value> {
    update_status(root, id, expected_revision, "deleted")
}

pub fn restore(root: &str, id: &str, expected_revision: &str) -> R<Value> {
    update_status(root, id, expected_revision, "active")
}

fn update_status(root: &str, id: &str, expected_revision: &str, status: &str) -> R<Value> {
    let root_path = root_path(root)?; let _lock = LibLock::acquire(&root_path, 30)?; let current = read(root_path.to_string_lossy().as_ref(), id)?;
    if version_of(&current) != expected_revision { return Err(err("记忆 revision 冲突")); }
    let plane = text(&current, "plane").ok_or_else(|| err("记忆记录缺少 plane"))?;
    let mut map = match current.clone() { Value::Object(m) => m, _ => return Err(err("记忆记录必须是对象")) };
    let next = version_of(&current).parse::<u64>().unwrap_or(1).saturating_add(1);
    map.insert("status".into(), Value::s(status)); map.insert("revision".into(), Value::n(next.to_string())); map.insert("updated_at".into(), Value::n(now().to_string()));
    let next_value = Value::Object(map); let path = record_path(&root_path, &plane, id)?; crate::util::write_atomic_cas(&path, &mcp_json::stringify(&next_value), &mcp_json::stringify(&current))?; Ok(next_value)
}
