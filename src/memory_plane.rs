//! 六个普通记忆平面的最小结构化运行时内核；credentials 仍由 vault 独立管理。
use crate::mcp_json::{self, Value};
use crate::util::{write_atomic, EE, LibLock, R};
use std::collections::{BTreeMap, BTreeSet};
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
fn legacy_value(path: &Path, plane: &str, raw: &str) -> R<Value> {
    let record = crate::rec::Record::parse_str(raw, path, crate::rec::Lib::Canonical)?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let legacy_id = record.get("ID");
    let id = if valid_id(&legacy_id) { legacy_id } else { stem };
    if !valid_id(&id) { return Err(err(format!("旧迁移记录文件名不是合法 ID: {}", path.display()))); }
    let title = record.get("TITLE");
    let read_when = record.get("READ_WHEN");
    let status = if record.get("STATUS").eq_ignore_ascii_case("deleted") { "deleted" } else { "active" };
    let revision = record.get("MEMORY_REVISION").parse::<u64>().unwrap_or(1);
    let relation_keys = ["RELATED_IDS", "PROMOTED_FROM", "PROMOTED_TO", "SUPERSEDES", "PREVIOUS_ID", "MERGED_INTO"];
    let mut relations = Vec::new();
    for key in relation_keys {
        let value = record.get(key);
        for part in value.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).filter(|part| valid_id(part)) {
            relations.push(object(vec![("kind", Value::s(key)), ("target", Value::s(part))]));
        }
    }
    Ok(object(vec![
        ("id", Value::s(id.clone())), ("plane", Value::s(plane)), ("title", Value::s(title)),
        ("content", Value::s(record.body)), ("read_when", Value::s(read_when)),
        ("tags", Value::arr(Vec::new())), ("entities", Value::arr(Vec::new())),
        ("source", Value::s("legacy_migration")), ("confidence", Value::s("migrated")),
        ("created_at", Value::n(record.mtime.to_string())), ("updated_at", Value::n(record.mtime.to_string())),
        ("relations", Value::Array(relations)), ("legacy_ids", Value::arr(vec![Value::s(id.clone())])),
        ("legacy_fields", Value::Array(record.fields.iter().map(|(key, value)| object(vec![("key", Value::s(key)), ("value", Value::s(value))])).collect())),
        ("legacy_attach", record.attach.as_deref().map(Value::s).unwrap_or(Value::Null)),
        ("legacy_promotion", record.promotion.as_deref().map(Value::s).unwrap_or(Value::Null)),
        ("legacy_raw_sha256", Value::s(crate::sha256::digest_hex(raw.as_bytes()))),
        ("revision", Value::n(revision.to_string())), ("status", Value::s(status)),
    ]))
}
fn legacy_update(located: &LocatedRecord, expected: &Value, title: Option<&str>, read_when: Option<&str>, content: Option<&str>, status: Option<&str>) -> R<Value> {
    let raw = located.legacy_raw.as_ref().ok_or_else(|| err("不是旧迁移记录"))?;
    let mut record = crate::rec::Record::parse_str(raw, &located.path, crate::rec::Lib::Canonical)?;
    let revision = version_of(expected).parse::<u64>().unwrap_or(1).saturating_add(1);
    record.set("MEMORY_REVISION", &revision.to_string());
    if let Some(value) = title { record.set("TITLE", value); }
    if let Some(value) = read_when { record.set("READ_WHEN", value); }
    if let Some(value) = content { record.body = value.to_string(); }
    if let Some(value) = status { record.set("STATUS", value); }
    let rendered = record.render_for_write();
    crate::util::write_atomic_cas(&located.path, &rendered, raw)?;
    legacy_value(&located.path, &located.plane, &rendered)
}
fn resolve_legacy_alias(root: &Path, alias: &str) -> R<Option<Value>> {
    let index = root.join("legacy_aliases.jsonl");
    let meta = match fs::symlink_metadata(&index) { Ok(meta) => meta, Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None), Err(e) => return Err(e.into()) };
    if meta.file_type().is_symlink() || !meta.is_file() { return Err(err("旧身份映射必须是普通文件")); }
    let mut targets = BTreeSet::new();
    for line in fs::read_to_string(&index)?.lines() {
        let value = mcp_json::parse(line).map_err(|e| err(format!("旧身份映射 JSON 无效: {e}")))?;
        if text(&value, "alias").as_deref() == Some(alias) {
            if let Some(target) = text(&value, "target_rel") { targets.insert(target); }
        }
    }
    if targets.len() > 1 { return Err(err(format!("旧身份别名映射到多个记录: {alias}"))); }
    let Some(target) = targets.into_iter().next() else { return Ok(None); };
    let rel = Path::new(&target);
    if rel.is_absolute() || rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) { return Err(err("旧身份映射目标路径非法")); }
    let path = fs::canonicalize(root.join(rel)).map_err(|e| err(format!("旧身份映射目标缺失: {e}")))?;
    if !path.starts_with(root) { return Err(err("旧身份映射目标越出库根")); }
    let plane = rel.components().nth(1).and_then(|c| c.as_os_str().to_str()).ok_or_else(|| err("旧身份映射目标缺 plane"))?;
    if !plane_ok(plane) { return Err(err(format!("旧身份映射目标 plane 非法: {plane}"))); }
    let raw = fs::read_to_string(&path)?;
    let value = match path.extension().and_then(|s| s.to_str()) {
        Some("md") => legacy_value(&path, plane, &raw)?,
        Some("json") => mcp_json::parse(&raw).map_err(|e| err(format!("旧身份目标 JSON 无效: {e}")))?,
        _ => return Err(err("旧身份映射目标记录类型不支持")),
    };
    Ok(Some(value))
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
            if !meta.is_file() { continue; }
            let raw = fs::read_to_string(&path)?;
            let value = match path.extension().and_then(|s| s.to_str()) {
                Some("json") => mcp_json::parse(&raw).map_err(|e| err(format!("解析记忆记录失败 {}: {e}", path.display())))?,
                Some("md") => legacy_value(&path, p, &raw)?,
                _ => continue,
            };
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

struct LocatedRecord { plane: String, path: PathBuf, value: Value, legacy_raw: Option<String> }

fn locate_record(root: &Path, id: &str) -> R<Option<LocatedRecord>> {
    let mut found = Vec::new();
    for plane in PLANES {
        let dir = ensure_plane_dirs(root, plane)?;
        if !dir.is_dir() { continue; }
        let mut entries: Vec<_> = fs::read_dir(&dir)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path(); let meta = fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() { return Err(err(format!("记忆目录含符号链接: {}", path.display()))); }
            if !meta.is_file() { continue; }
            let raw = fs::read_to_string(&path)?;
            let value = match path.extension().and_then(|s| s.to_str()) {
                Some("json") => mcp_json::parse(&raw).map_err(|e| err(format!("记忆记录 JSON 无效 {}: {e}", path.display())))?,
                Some("md") => legacy_value(&path, plane, &raw)?,
                _ => continue,
            };
            if text(&value, "id").as_deref() == Some(id) { found.push(LocatedRecord { plane: (*plane).to_string(), path, value, legacy_raw: if entry.path().extension().and_then(|s| s.to_str()) == Some("md") { Some(raw) } else { None } }); }
        }
    }
    match found.len() { 0 => Ok(None), 1 => Ok(found.pop()), _ => Err(err(format!("记忆 ID 跨平面歧义: {id}"))) }
}

pub fn read(root: &str, id: &str) -> R<Value> {
    if !valid_id(id) { return Err(err("非法 memory record id")); }
    let root = root_path(root)?;
    if let Some(record) = locate_record(&root, id)? { return Ok(record.value); }
    if let Some(alias) = resolve_legacy_alias(&root, id)? { return Ok(alias); }
    crate::util::notfound::<Value>(format!("没有找到记忆记录: {id}"))
}

pub fn search(root: &str, query: &str, plane: Option<&str>, limit: usize) -> R<Value> {
    if query.trim().is_empty() { return Err(err("memory_search 需要非空 query")); }
    let root = root_path(root)?;
    let needle = query.to_lowercase();
    let mut hits = Vec::new();
    for item in records(&root, plane)? {
        let hay = [text(&item, "title"), text(&item, "content"), text(&item, "read_when")].into_iter().flatten().collect::<Vec<_>>().join("\n").to_lowercase();
        if hay.contains(&needle) { hits.push(item); }
    }
    hits.sort_by_key(|v| text(v, "id").unwrap_or_default());
    let total = hits.len();
    let size = limit.clamp(1, 500);
    hits.truncate(size);
    Ok(object(vec![("items", Value::Array(hits)), ("total", Value::n(total.to_string())), ("limit", Value::n(size.to_string()))]))
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


pub fn resolve_attachment_record_path(root: &Path, selector: &str) -> R<PathBuf> {
    let direct = Path::new(selector);
    let path = if direct.is_absolute() { direct.to_path_buf() } else { root.join(direct) };
    if path.exists() {
        let canonical = fs::canonicalize(&path).map_err(|e| err(format!("附件记录路径无法解析: {e}")))?;
        if !canonical.starts_with(root) { return Err(err("附件记录路径越出库根")); }
        if !matches!(canonical.extension().and_then(|s| s.to_str()), Some("md" | "json")) { return Err(err("附件记录文件后缀只支持 md/json")); }
        return Ok(canonical);
    }
    if !valid_id(selector) { return Err(err("附件记录 selector 不是合法 ID 或相对路径")); }
    if let Some(located) = locate_record(root, selector)? { return Ok(located.path); }
    crate::util::notfound::<PathBuf>(format!("没有找到附件记录: {selector}"))
}

pub fn commit_attachment_manifest(path: &Path, manifest: &str) -> R<bool> {
    if path.extension().and_then(|s| s.to_str()) == Some("md") {
        let mut record = crate::rec::Record::parse(path, crate::rec::Lib::Canonical)?;
        record.attach = if manifest.trim().is_empty() { None } else { Some(manifest.to_string()) };
        let parent = path.parent().ok_or_else(|| err("附件记录缺少父目录"))?;
        let root = if parent.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str()) == Some("planes") {
            parent.parent().and_then(|p| p.parent()).ok_or_else(|| err("无法解析平面库根"))?
        } else {
            parent.parent().and_then(|p| p.parent()).ok_or_else(|| err("无法解析旧库根"))?
        };
        let args = crate::util::Args { cmd: String::new(), pos: Vec::new(), kv: BTreeMap::from([("root".to_string(), vec![root.to_string_lossy().to_string()])]) };
        crate::cmd_edit::save(&crate::util::resolve_root(&args), &mut record, &crate::util::Args::empty(), "attach")?;
        return Ok(true);
    }
    let raw = fs::read_to_string(path)?;
    let mut value = mcp_json::parse(&raw).map_err(|e| err(format!("附件记录 JSON 无效: {e}")))?;
    let next_revision = text(&value, "revision").and_then(|s| s.parse::<u64>().ok()).unwrap_or(1).saturating_add(1);
    let map = match &mut value { Value::Object(map) => map, _ => return Err(err("附件记录 JSON 必须为对象")) };
    map.insert("attachment_manifest".into(), Value::s(manifest));
    map.insert("revision".into(), Value::n(next_revision.to_string()));
    map.insert("updated_at".into(), Value::n(now().to_string()));
    crate::util::write_atomic_cas(path, &mcp_json::stringify(&value), &raw)?;
    Ok(true)
}

pub fn attachment_record(path: &Path) -> R<crate::rec::Record> {
    let raw = fs::read_to_string(path)?;
    let value = mcp_json::parse(&raw).map_err(|e| err(format!("附件记录 JSON 无效: {e}")))?;
    let fields = [("ID", "id"), ("TITLE", "title"), ("READ_WHEN", "read_when")].iter().filter_map(|(field, key)| text(&value, key).map(|v| ((*field).to_string(), v))).collect();
    let attach = text(&value, "attachment_manifest").or_else(|| text(&value, "legacy_attach"));
    Ok(crate::rec::Record { path: path.to_path_buf(), lib: crate::rec::Lib::Canonical, fields, body: text(&value, "content").unwrap_or_default(), promotion: None, attach, raw, devs: Vec::new(), header_inferred: false, base: None, mtime: fs::metadata(path).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0) })
}


mod legacy_migration_tests {
    use super::*;

    fn temp_root() -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("abook-memory-legacy-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn staged_markdown_record_and_alias_can_be_read_updated_deleted_and_restored() {
        let root = temp_root();
        let dir = root.join("planes/long_term/records");
        fs::create_dir_all(&dir).unwrap();
        let id = "C-20261101T120000-aaaaaaaa";
        let path = dir.join("canonical-legacyhash.md");
        let raw = format!("[CANONICAL_MEMORY]\nID={id}\nTITLE=Legacy title\nREAD_WHEN=legacy lookup\nX_UNKNOWN=preserve me\n[/CANONICAL_MEMORY]\n[MEMORY_BODY]\nLegacy body\n[/MEMORY_BODY]\n");
        fs::write(&path, raw).unwrap();
        let alias = "N-20261101T120000-bbbbbbbb";
        let map = format!("{{\"alias\":{},\"target_rel\":{},\"sources\":[]}}\n", mcp_json::stringify(&Value::s(alias)), mcp_json::stringify(&Value::s("planes/long_term/records/canonical-legacyhash.md")));
        fs::write(root.join("legacy_aliases.jsonl"), map).unwrap();
        let read = read(root.to_string_lossy().as_ref(), alias).unwrap();
        assert_eq!(text(&read, "content").as_deref(), Some("Legacy body"));
        let updated = update(root.to_string_lossy().as_ref(), id, "1", None, None, Some("updated body\nwith newline")).unwrap();
        assert_eq!(text(&updated, "content").as_deref(), Some("updated body\nwith newline"));
        assert_eq!(text(&updated, "revision").as_deref(), Some("2"));
        assert!(fs::read_to_string(&path).unwrap().contains("X_UNKNOWN=preserve me"));
        let deleted = delete(root.to_string_lossy().as_ref(), id, "2").unwrap();
        assert_eq!(text(&deleted, "status").as_deref(), Some("deleted"));
        assert!(list(root.to_string_lossy().as_ref(), "long_term", 10, 0).unwrap().get("total").and_then(|v| match v { Value::Number(n) => n.parse::<u64>().ok(), _ => None }) == Some(0));
        let restored = restore(root.to_string_lossy().as_ref(), id, "3").unwrap();
        assert_eq!(text(&restored, "status").as_deref(), Some("active"));
        let _ = fs::remove_dir_all(root);
    }
}
fn version_of(value: &Value) -> String { text(value, "revision").unwrap_or_else(|| "1".to_string()) }

pub fn update(root: &str, id: &str, expected_revision: &str, title: Option<&str>, read_when: Option<&str>, content: Option<&str>) -> R<Value> {
    let root = root_path(root)?; let _lock = LibLock::acquire(&root, 30)?;
    let located = locate_record(&root, id)?.ok_or_else(|| err(format!("没有找到记忆记录: {id}")))?;
    let current = located.value.clone();
    if version_of(&current) != expected_revision { return Err(err(format!("记忆 revision 冲突: expected={expected_revision} actual={}", version_of(&current)))); }
    if located.legacy_raw.is_some() { return legacy_update(&located, &current, title, read_when, content, None); }
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
    let root = root_path(root)?; let _lock = LibLock::acquire(&root, 30)?; let located = locate_record(&root, id)?.ok_or_else(|| err(format!("没有找到记忆记录: {id}")))?;
    let current = located.value.clone();
    if version_of(&current) != expected_revision { return Err(err("记忆 revision 冲突")); }
    if located.legacy_raw.is_some() { return legacy_update(&located, &current, None, None, None, Some(status)); }
    let plane = text(&current, "plane").ok_or_else(|| err("记忆记录缺少 plane"))?;
    let mut map = match current.clone() { Value::Object(m) => m, _ => return Err(err("记忆记录必须是对象")) };
    let next = version_of(&current).parse::<u64>().unwrap_or(1).saturating_add(1);
    map.insert("status".into(), Value::s(status)); map.insert("revision".into(), Value::n(next.to_string())); map.insert("updated_at".into(), Value::n(now().to_string()));
    let next_value = Value::Object(map); let path = record_path(&root, &plane, id)?; crate::util::write_atomic_cas(&path, &mcp_json::stringify(&next_value), &mcp_json::stringify(&current))?; Ok(next_value)
}
