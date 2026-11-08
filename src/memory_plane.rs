//! 六个普通记忆平面的最小结构化运行时内核；credentials 仍由 vault 独立管理。
use crate::mcp_json::{self, Value};
use crate::util::{write_atomic, EE, LibLock, R};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PLANES: &[&str] = &["soul", "rules", "profile", "explicit", "long_term", "episodic"];
pub const LAYOUT_MARKER: &str = "abook.memory.layout.v1";

fn err(message: impl Into<String>) -> EE { match crate::util::integrity::<()>(message) { Err(e) => e, Ok(()) => unreachable!() } }
/// 合法平面目录：六个普通平面，加上迁移器产生的墓碑目录。
///
/// `tombstones` 不是普通平面——它不参与检索、不出现在 PLANES 的枚举里，
/// 但迁移器会把软删记录写到 `planes/tombstones/records/`。如果这里不认它，
/// 含软删记录的库根本建不了迁移树，报"记录平面与目标路径不匹配"。
pub const TOMBSTONE_PLANE: &str = "tombstones";

fn plane_ok(plane: &str) -> bool { PLANES.contains(&plane) || plane == TOMBSTONE_PLANE }

pub fn verify_layout(root: &Path) -> R<()> {
    let marker = root.join(LAYOUT_MARKER);
    let meta = fs::symlink_metadata(&marker).map_err(|_| err(format!("六平面布局缺少 marker: {}", marker.display())))?;
    if meta.file_type().is_symlink() || !meta.is_file() { return Err(err("六平面布局 marker 必须是普通文件")); }
    let value = fs::read_to_string(&marker)?;
    if value.trim_end() != LAYOUT_MARKER { return Err(err("六平面布局 marker 版本无效")); }
    for legacy in ["canonical", "candidate", "Global.md", "Index.md", "Atlas.md", ".numbers"] {
        let path = root.join(legacy);
        if path.exists() { return Err(err(format!("六平面布局混入旧模型标记: {}", path.display()))); }
    }
    for plane in PLANES {
        for dir in [root.join("planes").join(plane), root.join("planes").join(plane).join("records")] {
            let meta = fs::symlink_metadata(&dir).map_err(|_| err(format!("六平面布局不完整，缺少目录: {}", dir.display())))?;
            if meta.file_type().is_symlink() || !meta.is_dir() { return Err(err(format!("六平面布局目录类型无效: {}", dir.display()))); }
        }
    }
    Ok(())
}

fn ensure_layout(root: &Path) -> R<()> {
    fs::create_dir_all(root)?;
    let marker = root.join(LAYOUT_MARKER);
    if marker.exists() { return verify_layout(root); }
    for legacy in ["canonical", "candidate", "Global.md", "Index.md", "Atlas.md", ".numbers"] {
        let path = root.join(legacy);
        if path.exists() { return Err(err(format!("旧/未完成记忆库不能直接作为六平面 memory 根: {}", path.display()))); }
    }
    if root.join("planes").exists() { return Err(err("发现 planes 目录但缺少版本 marker，拒绝猜测库格式")); }
    let planes_root = root.join("planes");
    let creation = (|| -> R<()> {
        for plane in PLANES { fs::create_dir_all(planes_root.join(plane).join("records"))?; }
        write_atomic(&marker, &format!("{LAYOUT_MARKER}\n"))?;
        verify_layout(root)
    })();
    if let Err(error) = creation {
        let _ = fs::remove_file(&marker);
        let _ = fs::remove_dir_all(&planes_root);
        return Err(error);
    }
    Ok(())
}

pub fn detect_layout(root: &Path) -> R<bool> {
    if !root.exists() { return Ok(false); }
    let metadata = fs::symlink_metadata(root)?;
    let canonical;
    let root = if metadata.file_type().is_symlink() { canonical = fs::canonicalize(root)?; canonical.as_path() } else { root };
    if !root.is_dir() { return Err(err("库根必须是目录")); }
    let marker = root.join(LAYOUT_MARKER);
    if marker.exists() { verify_layout(root)?; return Ok(true); }
    if root.join("planes").exists() { return Err(err("发现 planes 目录但缺少版本 marker，拒绝猜测库格式")); }
    Ok(false)
}

pub fn initialize_layout(root: &str) -> R<()> {
    let root = resolve_memory_path(root)?;
    let _lock = LibLock::acquire(&root, 30)?;
    ensure_layout(&root)
}

/// 解析记忆库根。
///
/// 空 `root` 不是错误：abook 二进制与记忆库同层放置，程序自己就知道库在哪，
/// 因此缺省时走与 CLI 完全相同的定位链（显式 root → 程序所在目录 →
/// `ABOOK_ROOT` → `~/AgentsBook`）。要求调用方每次传 root 是把工具的内部
/// 约定外泄给使用者——真实使用里每调一次 MCP 工具都要重复粘贴一个路径，
/// 既啰嗦又容易指错目录。
///
/// 注意不能简单用"当前工作目录"兜底：cwd 与库根无关，用错地方会静默
/// 把记录写到别处，比报错更糟。
fn resolve_memory_path(root: &str) -> R<PathBuf> {
    let args = if root.is_empty() {
        crate::util::Args { cmd: String::new(), pos: Vec::new(), kv: BTreeMap::new() }
    } else {
        crate::util::Args { cmd: String::new(), pos: Vec::new(), kv: BTreeMap::from([("root".to_string(), vec![root.to_string()])]) }
    };
    let resolved = crate::util::resolve_root(&args);
    if resolved.exists() {
        let meta = fs::symlink_metadata(&resolved)?;
        if meta.file_type().is_symlink() || !meta.is_dir() { return Err(err("memory root 必须是普通目录")); }
        Ok(fs::canonicalize(resolved)?)
    } else {
        Ok(resolved)
    }
}

fn root_path(root: &str) -> R<PathBuf> {
    let resolved = resolve_memory_path(root)?;
    if !resolved.exists() { return Err(err(format!("六平面 memory 根不存在，请先执行 memory init: {}", resolved.display()))); }
    verify_layout(&resolved)?;
    Ok(resolved)
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
fn legacy_update(located: &LocatedRecord, expected: &Value, title: Option<&str>, read_when: Option<&str>, content: Option<&str>, status: Option<&str>, fields: &[(String, String)]) -> R<Value> {
    let raw = located.legacy_raw.as_ref().ok_or_else(|| err("不是旧迁移记录"))?;
    let mut record = crate::rec::Record::parse_str(raw, &located.path, crate::rec::Lib::Canonical)?;
    let revision = version_of(expected).parse::<u64>().unwrap_or(1).saturating_add(1);
    record.set("MEMORY_REVISION", &revision.to_string());
    if let Some(value) = title { record.set("TITLE", value); }
    if let Some(value) = read_when { record.set("READ_WHEN", value); }
    if let Some(value) = content { record.body = value.to_string(); }
    if let Some(value) = status { record.set("STATUS", value); }
    // 任意头部字段：KIND/MATURITY/AUTHORITY/KEYS/NOT_FOR/ENTITIES 这些原本没有
    // 任何写入路径，想改只能删掉重写，那会断掉 ID 与关系链接。这里按调用方
    // 给的键值逐个写入；空值表示清除该字段。
    for (key, value) in fields {
        let upper = key.trim().to_ascii_uppercase();
        if upper.is_empty() || upper == "ID" {
            // ID 是身份，改它等于换一条记录，不在字段编辑范围内。
            return Err(err("字段编辑不接受 ID；改身份请用迁移工具"));
        }
        record.set(&upper, value);
    }
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
fn records_with_status(root: &Path, plane: Option<&str>, include_deleted: bool) -> R<Vec<Value>> {
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
            if include_deleted || text(&value, "status").as_deref() != Some("deleted") { out.push(value); }
        }
    }
    Ok(out)
}

fn records(root: &Path, plane: Option<&str>) -> R<Vec<Value>> { records_with_status(root, plane, false) }

fn preview(content: &str, max_chars: usize) -> (String, usize, bool) {
    let total_chars = content.chars().count();
    let value: String = content.chars().take(max_chars).collect();
    (value, total_chars, total_chars > max_chars)
}

pub fn index(root: &str, plane: Option<&str>, limit: usize, offset: usize) -> R<Value> {
    let root = root_path(root)?;
    let selected: Vec<&str> = match plane {
        Some("all") | None => PLANES.to_vec(),
        Some(p) if plane_ok(p) => vec![p],
        Some(p) => return Err(err(format!("未知记忆平面: {p}"))),
    };
    let all = records_with_status(&root, plane.filter(|p| *p != "all"), true)?;
    let mut counts = BTreeMap::new();
    for p in &selected {
        let mut total = 0usize;
        let mut active = 0usize;
        let mut deleted = 0usize;
        for value in &all {
            if text(value, "plane").as_deref() == Some(*p) {
                total += 1;
                if text(value, "status").as_deref() == Some("deleted") { deleted += 1; } else { active += 1; }
            }
        }
        counts.insert((*p).to_string(), object(vec![
            ("total", Value::n(total.to_string())),
            ("active", Value::n(active.to_string())),
            ("deleted", Value::n(deleted.to_string())),
        ]));
    }
    let mut items: Vec<Value> = all.iter().filter(|v| selected.contains(&text(v, "plane").as_deref().unwrap_or(""))).map(|v| {
        let (text_preview, chars, truncated) = preview(&text(v, "content").unwrap_or_default(), 160);
        object(vec![
            ("id", Value::s(text(v, "id").unwrap_or_default())),
            ("plane", Value::s(text(v, "plane").unwrap_or_default())),
            ("title", Value::s(text(v, "title").unwrap_or_default())),
            ("read_when", Value::s(text(v, "read_when").unwrap_or_default())),
            ("updated_at", Value::s(text(v, "updated_at").unwrap_or_default())),
            ("revision", Value::s(text(v, "revision").unwrap_or_else(|| "1".into()))),
            ("status", Value::s(text(v, "status").unwrap_or_else(|| "active".into()))),
            ("preview", Value::s(text_preview)),
            ("preview_chars", Value::n(chars.to_string())),
            ("preview_truncated", Value::b(truncated)),
        ])
    }).collect();
    items.sort_by(|a,b| text(a,"plane").cmp(&text(b,"plane")).then_with(|| text(a,"id").cmp(&text(b,"id"))));
    let total = items.len();
    let limit = limit.clamp(1, 500);
    let effective_offset = offset.min(total);
    let page: Vec<Value> = items.into_iter().skip(effective_offset).take(limit).collect();
    Ok(object(vec![
        ("format", Value::s("abook.memory.index.v1")),
        ("planes", Value::arr(selected.iter().map(|p| Value::s(*p)).collect())),
        ("counts", Value::Object(counts)),
        ("items", Value::Array(page)),
        ("total", Value::n(total.to_string())),
        ("offset", Value::n(effective_offset.to_string())),
        ("limit", Value::n(limit.to_string())),
        ("credentials", Value::s("vault")),
    ]))
}

pub fn write(root: &str, plane: &str, title: &str, read_when: &str, content: &str) -> R<Value> {
    if !plane_ok(plane) { return Err(err(format!("未知记忆平面: {plane}"))); }
    if title.trim().is_empty() || content.is_empty() { return Err(err("memory_write 需要非空 title 和 content")); }
    let root = resolve_memory_path(root)?;
    let _lock = LibLock::acquire(&root, 30)?;
    ensure_layout(&root)?;
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

/// 检索字段选择。
///
/// 原先只能整条模糊比对，想只搜标题或只搜 KEYS 没有出口——那会逼调用方
/// 先取全量再自己过滤，等于把检索成本转嫁出去。
#[derive(Clone, Copy, PartialEq)]
pub enum SearchFields {
    All,
    Title,
    Content,
    ReadWhen,
}

pub fn search_filtered(
    root: &str,
    query: &str,
    plane: Option<&str>,
    limit: usize,
    fields: SearchFields,
    regex: bool,
) -> R<Value> {
    if query.trim().is_empty() { return Err(err("memory_search 需要非空 query")); }
    let root = root_path(root)?;
    let needle = query.to_lowercase();
    let matcher = if regex { Some(compile_lite_regex(&needle)?) } else { None };
    // 闭包只读捕获 matcher/needle，与 hits 的可变借用互不干扰。
    let matches = |hay: &str| -> bool {
        match &matcher {
            Some(program) => program.is_match(hay),
            None => hay.contains(&needle),
        }
    };
    let mut hits = Vec::new();
    for item in records(&root, plane)? {
        let hay = match fields {
            SearchFields::All => [text(&item, "title"), text(&item, "content"), text(&item, "read_when")]
                .into_iter().flatten().collect::<Vec<_>>().join("\n"),
            SearchFields::Title => text(&item, "title").unwrap_or_default(),
            SearchFields::Content => text(&item, "content").unwrap_or_default(),
            SearchFields::ReadWhen => text(&item, "read_when").unwrap_or_default(),
        };
        if matches(&hay.to_lowercase()) { hits.push(item); }
    }
    hits.sort_by_key(|v| text(v, "id").unwrap_or_default());
    // 说明：下面这段是 search_filtered 的公共尾部，旧 search 通过它复用。
    let total = hits.len();
    let size = limit.clamp(1, 500);
    hits.truncate(size);
    Ok(object(vec![("items", Value::Array(hits)), ("total", Value::n(total.to_string())), ("limit", Value::n(size.to_string()))]))
}

/// 兼容入口：不带字段/正则参数的检索。
pub fn search(root: &str, query: &str, plane: Option<&str>, limit: usize) -> R<Value> {
    search_filtered(root, query, plane, limit, SearchFields::All, false)
}

/// 列表过滤条件。
///
/// `status` 缺省只看 active——这是原先的行为，但那时无法反过来查看软删记录，
/// 于是"回收站"在 MCP 面等于不存在：`memory_restore` 有，却没有地方先看见
/// 被删的条目。`include_deleted` 打开后即可当回收站用。
#[derive(Default, Clone)]
pub struct ListFilter {
    pub status: Option<String>,
    pub kind: Option<String>,
    pub include_deleted: bool,
}

pub fn list_filtered(
    root: &str,
    plane: &str,
    limit: usize,
    offset: usize,
    filter: &ListFilter,
) -> R<Value> {
    if plane != "all" && !plane_ok(plane) { return Err(err(format!("未知记忆平面: {plane}"))); }
    let root = root_path(root)?;
    let selected = if plane == "all" { None } else { Some(plane) };
    let include_deleted = filter.include_deleted
        || filter
            .status
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case("deleted"))
            .unwrap_or(false);
    let mut items = records_with_status(&root, selected, include_deleted)?;
    if let Some(want) = &filter.status {
        let want = want.to_ascii_lowercase();
        items.retain(|item| {
            text(item, "status")
                .unwrap_or_else(|| "active".to_string())
                .eq_ignore_ascii_case(&want)
        });
    }
    if let Some(want) = &filter.kind {
        let want = want.to_ascii_lowercase();
        items.retain(|item| {
            text(item, "kind")
                .map(|k| k.eq_ignore_ascii_case(&want))
                .unwrap_or(false)
        });
    }
    items.sort_by(|a,b| text(a, "plane").cmp(&text(b, "plane")).then_with(|| text(a, "id").cmp(&text(b, "id"))));
    let total = items.len();
    let start = offset.min(total);
    let size = limit.clamp(1, 500);
    let page = items.into_iter().skip(start).take(size).collect();
    Ok(object(vec![
        ("items", Value::Array(page)),
        ("planes", Value::arr(if plane == "all" { PLANES.iter().map(|p| Value::s(*p)).collect() } else { vec![Value::s(plane)] })),
        ("total", Value::n(total.to_string())),
        ("offset", Value::n(start.to_string())),
        ("limit", Value::n(size.to_string())),
        ("credentials", Value::s("vault")),
    ]))
}

pub fn list(root: &str, plane: &str, limit: usize, offset: usize) -> R<Value> {
    if plane != "all" && !plane_ok(plane) { return Err(err(format!("未知记忆平面: {plane}"))); }
    let root = root_path(root)?;
    let selected = if plane == "all" { None } else { Some(plane) };
    let mut items = records(&root, selected)?;
    items.sort_by(|a,b| text(a, "plane").cmp(&text(b, "plane")).then_with(|| text(a, "id").cmp(&text(b, "id"))));
    let total = items.len();
    let start = offset.min(total);
    let size = limit.clamp(1, 500);
    let page = items.into_iter().skip(start).take(size).collect();
    Ok(object(vec![("items", Value::Array(page)), ("planes", Value::arr(if plane == "all" { PLANES.iter().map(|p| Value::s(*p)).collect() } else { vec![Value::s(plane)] })), ("total", Value::n(total.to_string())), ("offset", Value::n(start.to_string())), ("limit", Value::n(size.to_string())), ("credentials", Value::s("vault"))]))
}
pub fn context(root: &str) -> R<Value> {
    let root = root_path(root)?;
    let mut counts = BTreeMap::new();
    for plane in PLANES { counts.insert((*plane).to_string(), Value::n(records(&root, Some(plane))?.len().to_string())); }
    Ok(object(vec![("planes", Value::Object(counts)), ("credentials", Value::s("vault")), ("total", Value::n(records(&root, None)?.len().to_string()))]))
}

pub fn paths(root: &str) -> R<Value> {
    let root = root_path(root)?;
    let marker = root.join(LAYOUT_MARKER);
    let mut planes = BTreeMap::new();
    for plane in PLANES {
        let path = root.join("planes").join(plane).join("records");
        planes.insert((*plane).to_string(), object(vec![
            ("path", Value::s(path.display().to_string())),
            ("exists", Value::b(path.is_dir())),
        ]));
    }
    let aliases = root.join("legacy_aliases.jsonl");
    let tombstones = root.join("planes/tombstones/records");
    let vault = root.join("vault");
    Ok(object(vec![
        ("format", Value::s("abook.memory.paths.v1")),
        ("root", Value::s(root.display().to_string())),
        ("marker", object(vec![("path", Value::s(marker.display().to_string())), ("exists", Value::b(marker.is_file()))])),
        ("planes", Value::Object(planes)),
        ("aliases", object(vec![("path", Value::s(aliases.display().to_string())), ("exists", Value::b(aliases.is_file()))])),
        ("tombstones", object(vec![("path", Value::s(tombstones.display().to_string())), ("exists", Value::b(tombstones.is_dir()))])),
        ("vault", object(vec![("path", Value::s(vault.display().to_string())), ("exists", Value::b(vault.is_dir()))])),
        ("credentials", Value::s("vault")),
    ]))
}
pub fn export(root: &str, plane: Option<&str>) -> R<Value> {
    let selected = match plane {
        Some("all") | None => None,
        Some(p) if plane_ok(p) => Some(p),
        Some(p) => return Err(err(format!("未知记忆平面: {p}"))),
    };
    let root = root_path(root)?;
    let items = records(&root, selected)?;
    let mut counts = BTreeMap::new();
    for p in PLANES {
        if selected.map(|selected| selected == *p).unwrap_or(true) {
            counts.insert((*p).to_string(), Value::n(records(&root, Some(p))?.len().to_string()));
        }
    }
    let total = items.len();
    Ok(object(vec![
        ("format", Value::s("abook.memory.export.v1")),
        ("planes", Value::Object(counts)),
        ("items", Value::Array(items)),
        ("total", Value::n(total.to_string())),
        ("credentials", Value::s("vault")),
    ]))
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
    if path.extension().and_then(|s| s.to_str()) == Some("md") {
        return crate::rec::Record::parse(path, crate::rec::Lib::Canonical);
    }
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
    fn six_plane_attachment_manifest_scan_and_cas_roundtrip() {
        let root = temp_root();
        initialize_layout(root.to_string_lossy().as_ref()).unwrap();
        let record_dir = root.join("planes/explicit/records");
        let record = crate::memory_plane::write(root.to_string_lossy().as_ref(), "explicit", "附件测试", "查附件", "正文").unwrap();
        let indexed = index(root.to_string_lossy().as_ref(), Some("explicit"), 10, 0).unwrap();
        assert_eq!(text(&indexed, "format").as_deref(), Some("abook.memory.index.v1"));
        assert_eq!(text(&indexed, "total").as_deref(), Some("1"));
        assert!(root.join(LAYOUT_MARKER).is_file());
        assert!(!root.join("canonical").exists());
        let record_path = record_dir.join(format!("{}.json", text(&record, "id").unwrap()));
        let attachment_dir = crate::attach::dir_for(&record_path);

        fs::create_dir_all(&attachment_dir).unwrap();
        fs::write(attachment_dir.join("proof.txt"), "evidence").unwrap();
        let inventory = crate::attach::scan(&record_path);
        assert_eq!(inventory.items.len(), 1);
        let manifest = crate::attach::render_list(&inventory.items);
        commit_attachment_manifest(&record_path, &manifest).unwrap();
        let readback = attachment_record(&record_path).unwrap();
        assert_eq!(readback.attach.as_deref(), Some(manifest.as_str()));
        assert_eq!(crate::attach::verify(&record_path), crate::attach::Mismatch { missing: Vec::new(), extra: Vec::new(), resized: Vec::new(), edited: Vec::new() });
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn staged_markdown_record_and_alias_can_be_read_updated_deleted_and_restored() {
        let root = temp_root();
        initialize_layout(root.to_string_lossy().as_ref()).unwrap();
        let dir = root.join("planes/long_term/records");
        let id = "C-20261101T120000-aaaaaaaa";
        let path = dir.join("canonical-legacyhash.md");
        let raw = format!("[CANONICAL_MEMORY]\nID={id}\nTITLE=Legacy title\nREAD_WHEN=legacy lookup\nX_UNKNOWN=preserve me\n[/CANONICAL_MEMORY]\n[MEMORY_BODY]\nLegacy body\n[/MEMORY_BODY]\n");
        fs::write(&path, raw).unwrap();
        let alias = "N-20261101T120000-bbbbbbbb";
        let map = format!("{{\"alias\":{},\"target_rel\":{},\"sources\":[]}}\n", mcp_json::stringify(&Value::s(alias)), mcp_json::stringify(&Value::s("planes/long_term/records/canonical-legacyhash.md")));
        fs::write(root.join("legacy_aliases.jsonl"), map).unwrap();
        let read = read(root.to_string_lossy().as_ref(), alias).unwrap();
        assert_eq!(text(&read, "content").as_deref(), Some("Legacy body"));
        let updated = update(root.to_string_lossy().as_ref(), id, "1", None, None, Some("updated body\nwith newline"), &[]).unwrap();
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

    #[test]
    fn layout_marker_and_index_report_six_planes_and_tombstones() {
        let root = temp_root();
        let first = write(root.to_string_lossy().as_ref(), "explicit", "First", "index test", "x".repeat(200).as_str()).unwrap();
        let second = write(root.to_string_lossy().as_ref(), "profile", "Second", "index test", "body").unwrap();
        delete(root.to_string_lossy().as_ref(), &text(&first, "id").unwrap(), "1").unwrap();
        assert!(root.join(LAYOUT_MARKER).is_file());
        let indexed = index(root.to_string_lossy().as_ref(), None, 1, 0).unwrap();
        assert_eq!(text(&indexed, "format").as_deref(), Some("abook.memory.index.v1"));
        assert_eq!(text(&indexed, "total").as_deref(), Some("2"));
        assert_eq!(indexed.get("planes").and_then(|v| match v { Value::Array(a) => Some(a.len()), _ => None }), Some(6));
        let items = indexed.get("items").and_then(|v| match v { Value::Array(a) => Some(a), _ => None }).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(text(&items[0], "preview_chars").as_deref(), Some("200"));
        assert_eq!(items[0].get("preview_truncated"), Some(&Value::b(true)));
        let profile = index(root.to_string_lossy().as_ref(), Some("profile"), 10, 0).unwrap();
        assert_eq!(text(&profile, "total").as_deref(), Some("1"));
        assert!(index(root.to_string_lossy().as_ref(), Some("bogus"), 10, 0).is_err());
        assert_eq!(text(&second, "plane").as_deref(), Some("profile"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn paths_reports_marker_and_all_six_plane_dirs() {
        let root = temp_root();
        initialize_layout(root.to_string_lossy().as_ref()).unwrap();
        let value = paths(root.to_string_lossy().as_ref()).unwrap();
        assert_eq!(text(&value, "format").as_deref(), Some("abook.memory.paths.v1"));
        assert_eq!(text(&value, "credentials").as_deref(), Some("vault"));
        let planes = value.get("planes").and_then(|v| match v { Value::Object(map) => Some(map), _ => None }).unwrap();
        assert_eq!(planes.len(), PLANES.len());
        for plane in PLANES {
            let row = planes.get(*plane).unwrap();
            assert_eq!(row.get("exists"), Some(&Value::Bool(true)));
            assert!(Path::new(&text(row, "path").unwrap()).ends_with(Path::new("planes").join(plane).join("records")));
        }
        assert!(!root.join("canonical").exists());
        assert!(!root.join("candidate").exists());
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn refuses_to_initialize_memory_layout_on_legacy_root() {
        let root = temp_root();
        fs::create_dir_all(root.join("candidate/records")).unwrap();
        assert!(initialize_layout(root.to_string_lossy().as_ref()).is_err());
        assert!(!root.join(LAYOUT_MARKER).exists());
        let _ = fs::remove_dir_all(root);
    }
}


fn version_of(value: &Value) -> String { text(value, "revision").unwrap_or_else(|| "1".to_string()) }

pub fn update(root: &str, id: &str, expected_revision: &str, title: Option<&str>, read_when: Option<&str>, content: Option<&str>, fields: &[(String, String)]) -> R<Value> {
    let root = resolve_memory_path(root)?;
    let _lock = LibLock::acquire(&root, 30)?;
    verify_layout(&root)?;
    let located = locate_record(&root, id)?.ok_or_else(|| err(format!("没有找到记忆记录: {id}")))?;
    let current = located.value.clone();
    if version_of(&current) != expected_revision { return Err(err(format!("记忆 revision 冲突: expected={expected_revision} actual={}", version_of(&current)))); }
    if located.legacy_raw.is_some() { return legacy_update(&located, &current, title, read_when, content, None, fields); }
    let plane = text(&current, "plane").ok_or_else(|| err("记忆记录缺少 plane"))?;
    let mut map = match current.clone() { Value::Object(m) => m, _ => return Err(err("记忆记录必须是对象")) };
    if let Some(v) = title { map.insert("title".into(), Value::s(v)); }
    if let Some(v) = read_when { map.insert("read_when".into(), Value::s(v)); }
    if let Some(v) = content { map.insert("content".into(), Value::s(v)); }
    for (key, value) in fields {
        let normalized = key.trim().to_ascii_lowercase();
        if normalized.is_empty() || normalized == "id" {
            return Err(err("字段编辑不接受 id；改身份请用迁移工具"));
        }
        // revision/updated_at 由工具维护，调用方写它们会破坏并发保护。
        if matches!(normalized.as_str(), "revision" | "updated_at" | "created_at") {
            return Err(err(format!("字段 {normalized} 由工具维护，不接受直接编辑")));
        }
        map.insert(normalized, Value::s(value));
    }
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
    let root = resolve_memory_path(root)?; let _lock = LibLock::acquire(&root, 30)?; verify_layout(&root)?; let located = locate_record(&root, id)?.ok_or_else(|| err(format!("没有找到记忆记录: {id}")))?;
    let current = located.value.clone();
    if version_of(&current) != expected_revision { return Err(err("记忆 revision 冲突")); }
    if located.legacy_raw.is_some() { return legacy_update(&located, &current, None, None, None, Some(status), &[]); }
    let plane = text(&current, "plane").ok_or_else(|| err("记忆记录缺少 plane"))?;
    let mut map = match current.clone() { Value::Object(m) => m, _ => return Err(err("记忆记录必须是对象")) };
    let next = version_of(&current).parse::<u64>().unwrap_or(1).saturating_add(1);
    map.insert("status".into(), Value::s(status)); map.insert("revision".into(), Value::n(next.to_string())); map.insert("updated_at".into(), Value::n(now().to_string()));
    let next_value = Value::Object(map); let path = record_path(&root, &plane, id)?; crate::util::write_atomic_cas(&path, &mcp_json::stringify(&next_value), &mcp_json::stringify(&current))?;
    // 默认只回身份与状态：整条记录可能带几十项 legacy_fields，单条 3KB，
    // 批量删 20 条就吃掉 70KB 上下文，而调用方真正需要知道的只是"改成了什么状态、
    // 新版本号多少"。要全量再显式要。
    Ok(compact_status(&next_value))
}

fn compact_status(value: &Value) -> Value {
    object(vec![
        ("id", Value::s(text(value, "id").unwrap_or_default())),
        ("plane", Value::s(text(value, "plane").unwrap_or_default())),
        ("revision", Value::s(version_of(value))),
        ("status", Value::s(text(value, "status").unwrap_or_default())),
    ])
}

/// 拦住"会把整库写坏"的批量写入。
///
/// 背景是一个真实缺陷：`memory_batch_preview` 原本对 body 完全不校验，于是
/// 调用方把一段普通正文（比如 `标题\n正文 😀\n`）写进
/// `planes/explicit/records/x.md` 之后，这条文件既不是 JSON 也不是带字段的
/// 旧格式记录，`records()` 解析它必然报错——而 `records()` 是 list/context/
/// search/export 的共同入口，所以**一条坏文件让整个库读不了**。调用方只是想
/// "批量写个文件"，代价却是库变成不可用状态。
///
/// 因此凡是落在 `planes/<平面>/records/` 下的写入，正文必须能解析成一条记录；
/// 其他路径（配置、报告之类）不归这里管，照旧放行。
pub fn validate_record_path_and_body(root: &std::path::Path, path: &str, body: &str) -> R<()> {
    let rel = Path::new(path);
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    // 只认 `planes/<plane>/records/<file>` 这一种形状，其他路径放行。
    if parts.len() < 4 || parts[0] != "planes" || parts[2] != "records" {
        return Ok(());
    }
    if !plane_ok(&parts[1]) {
        return Err(err(format!("未知记忆平面: {}", parts[1])));
    }
    let ext = rel.extension().and_then(|s| s.to_str()).unwrap_or("");
    if ext != "md" && ext != "json" {
        return Err(err(format!(
            "记忆平面的记录必须是 .md 或 .json，不接受 .{ext}"
        )));
    }
    // 这里刻意不用 `root_path`：它要求库已初始化且布局合法，而批量/事务写入
    // 本身就可能发生在建库之前；真正的库状态由后续 stage/apply 负责判定。
    let root_abs = resolve_memory_path(root.to_string_lossy().as_ref())?;
    let full = root_abs.join(rel);
    if ext == "json" {
        mcp_json::parse(body)
            .map_err(|e| err(format!("记录内容不是合法 JSON，会使库无法读取: {e}")))?;
        return Ok(());
    }
    // .md 走与读取完全相同的解析器：能解析成记录才算通过，避免"写入放行、
    // 读取失败"这种自相矛盾的状态。
    legacy_value(&full, &parts[1], body)
        .map(|_| ())
        .map_err(|e| err(format!("记录内容无法解析为记忆记录，会使整库不可读: {}", e.msg())))
}

/// 跨平面移动一条记录。
///
/// 规范把平面归置当常规维护操作，但此前只有 delete + write 两条路可走，
/// 那样会换掉 ID、断掉 relations 与附件清单——等于把"归置"变成了"删掉重建"。
/// 这里保留 ID、revision 递增、其余字段原样搬运，只改 plane 归属。
///
/// 目标平面已有同 ID 记录时直接报错，不覆盖：那说明调用方指错了对象，
/// 静默覆盖会让一条真实记录凭空消失。
pub fn move_plane(root: &str, id: &str, to_plane: &str, expected_revision: &str) -> R<Value> {
    if !plane_ok(to_plane) { return Err(err(format!("未知记忆平面: {to_plane}"))); }
    let root = resolve_memory_path(root)?;
    let _lock = LibLock::acquire(&root, 30)?;
    verify_layout(&root)?;
    let located = locate_record(&root, id)?.ok_or_else(|| err(format!("没有找到记忆记录: {id}")))?;
    let current = located.value.clone();
    if version_of(&current) != expected_revision {
        return Err(err(format!(
            "记忆 revision 冲突: expected={expected_revision} actual={}",
            version_of(&current)
        )));
    }
    let from_plane = text(&current, "plane").ok_or_else(|| err("记忆记录缺少 plane"))?;
    if from_plane == to_plane {
        return Err(err(format!("记录已在 {to_plane} 平面，无需移动")));
    }
    if located.legacy_raw.is_some() {
        return Err(err(
            "旧迁移记录暂不支持直接跨平面移动：它的平面信息编码在文件路径上，直接搬会破坏 legacy_ids 映射",
        ));
    }
    let mut map = match current.clone() {
        Value::Object(m) => m,
        _ => return Err(err("记忆记录必须是对象")),
    };
    let next = version_of(&current).parse::<u64>().unwrap_or(1).saturating_add(1);
    map.insert("plane".into(), Value::s(to_plane));
    map.insert("revision".into(), Value::n(next.to_string()));
    map.insert("updated_at".into(), Value::n(now().to_string()));
    let moved = Value::Object(map);

    let dest = file_for(&root, to_plane, id)?;
    if dest.exists() {
        return Err(err(format!(
            "目标平面已有同 ID 记录，拒绝覆盖: {id} in {to_plane}"
        )));
    }
    let src = record_path(&root, &from_plane, id)?;
    // 目标是新建文件，不是覆盖：`write_atomic_cas` 的语义是"我读到的旧字节
    // 必须仍然在"，目标不存在时它会把新建判成"文件被删除后重建"而拒绝。
    // 这里用普通原子写，先写目标、成功后删源——中途失败时两个文件都在，
    // 后果只是多一条重复记录（可检测可清理），而不是凭空丢一条。
    crate::util::write_atomic(&dest, &mcp_json::stringify(&moved))?;
    fs::remove_file(&src)?;
    Ok(object(vec![
        ("id", Value::s(id)),
        ("from_plane", Value::s(from_plane)),
        ("plane", Value::s(to_plane)),
        ("revision", Value::n(next.to_string())),
        ("status", Value::s(text(&moved, "status").unwrap_or_else(|| "active".to_string()))),
    ]))
}

/// 轻量正则引擎：支持 `.`、`*`、`+`、`?`、`^`、`$`、字符类 `[abc]`、`[a-z]`、
/// 转义 `\d \w \s \.`，以及 `|` 分支。
///
/// 为什么自己写而不引 crate：这个项目的硬约束是单二进制、零第三方依赖，
/// 引 regex crate 会让产物和构建面都变大。而检索场景真正需要的是"够用的
/// 模式匹配"，不是完整的 PCRE 语义——不支持反向引用和前瞻，遇到不支持的
/// 语法直接报错，而不是静默给出错误结果（静默错配比不支持更糟）。
pub struct LiteRegex {
    branches: Vec<Vec<Token>>,
}

#[derive(Clone, Debug)]
enum Token {
    Literal(char),
    Any,
    Class { negated: bool, ranges: Vec<(char, char)> },
    Digit(bool),
    Word(bool),
    Space(bool),
    Start,
    End,
}

#[derive(Clone, Copy, PartialEq)]
enum Quant {
    One,
    ZeroOrMore,
    OneOrMore,
    Optional,
}

impl LiteRegex {
    pub fn new(pattern: &str) -> R<Self> {
        let mut branches: Vec<Vec<(Token, Quant)>> = Vec::new();
        for part in split_unescaped(pattern, '|') {
            branches.push(parse_branch(&part)?);
        }
        Ok(LiteRegex { branches })
    }

    pub fn is_match(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        self.branches.iter().any(|branch| branch_matches(branch, &chars))
    }
}

fn split_unescaped(pattern: &str, separator: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for ch in pattern.chars() {
        if escaped {
            current.push('\\');
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == separator {
            out.push(std::mem::take(&mut current));
            continue;
        }
        current.push(ch);
    }
    if escaped {
        current.push('\\');
    }
    out.push(current);
    out
}

fn parse_branch(pattern: &str) -> R<Vec<(Token, Quant)>> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut tokens: Vec<(Token, Quant)> = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let token = match ch {
            '.' => { index += 1; Token::Any }
            '^' => { index += 1; Token::Start }
            '$' => { index += 1; Token::End }
            '\\' => {
                let next = chars.get(index + 1).copied().ok_or_else(|| err("正则末尾的转义不完整"))?;
                index += 2;
                match next {
                    'd' => Token::Digit(false),
                    'D' => Token::Digit(true),
                    'w' => Token::Word(false),
                    'W' => Token::Word(true),
                    's' => Token::Space(false),
                    'S' => Token::Space(true),
                    other => Token::Literal(other),
                }
            }
            '[' => {
                let (token, consumed) = parse_class(&chars[index..])?;
                index += consumed;
                token
            }
            '*' | '+' | '?' => {
                return Err(err(format!("量词 {ch} 前面没有可重复的字符")));
            }
            other => { index += 1; Token::Literal(other) }
        };
        let quant = match chars.get(index).copied() {
            Some('*') => { index += 1; Quant::ZeroOrMore }
            Some('+') => { index += 1; Quant::OneOrMore }
            Some('?') => { index += 1; Quant::Optional }
            _ => Quant::One,
        };
        tokens.push((token, quant));
    }
    Ok(tokens)
}

fn parse_class(chars: &[char]) -> R<(Token, usize)> {
    let mut index = 1;
    let negated = matches!(chars.get(index), Some('^'));
    if negated { index += 1; }
    let mut ranges = Vec::new();
    let mut first = true;
    while index < chars.len() {
        let ch = chars[index];
        if ch == ']' && !first {
            return Ok((Token::Class { negated, ranges }, index + 1));
        }
        first = false;
        let low = if ch == '\\' {
            let next = chars.get(index + 1).copied().ok_or_else(|| err("字符类里转义不完整"))?;
            index += 2;
            match next {
                'd' => { ranges.push(('0', '9')); continue; }
                'w' => { ranges.push(('a', 'z')); ranges.push(('A', 'Z')); ranges.push(('0', '9')); ranges.push(('_', '_')); continue; }
                's' => { ranges.push((' ', ' ')); ranges.push(('\t', '\t')); continue; }
                other => other,
            }
        } else {
            index += 1;
            ch
        };
        if chars.get(index) == Some(&'-') && chars.get(index + 1).map(|c| *c != ']').unwrap_or(false) {
            let high = chars[index + 1];
            ranges.push((low, high));
            index += 2;
        } else {
            ranges.push((low, low));
        }
    }
    Err(err("字符类缺少右方括号"))
}

fn token_matches(token: &Token, ch: char) -> bool {
    match token {
        Token::Literal(want) => *want == ch,
        Token::Any => ch != '\n',
        Token::Class { negated, ranges } => {
            let inside = ranges.iter().any(|(low, high)| ch >= *low && ch <= *high);
            if *negated { !inside } else { inside }
        }
        Token::Digit(negated) => ch.is_ascii_digit() != *negated,
        Token::Word(negated) => (ch.is_alphanumeric() || ch == '_') != *negated,
        Token::Space(negated) => ch.is_whitespace() != *negated,
        Token::Start | Token::End => false,
    }
}

fn branch_matches(branch: &[(Token, Quant)], text: &[char]) -> bool {
    // 从每个位置尝试：模式未以 ^ 锚定时等价于"任意起点可匹配"。
    let anchored_start = matches!(branch.first().map(|(t, _)| t), Some(Token::Start));
    let starts: Vec<usize> = if anchored_start { vec![0] } else { (0..=text.len()).collect() };
    starts.into_iter().any(|start| match_from(branch, 0, text, start))
}

fn match_from(branch: &[(Token, Quant)], token_index: usize, text: &[char], position: usize) -> bool {
    if token_index == branch.len() {
        return true;
    }
    let (token, quant) = &branch[token_index];
    match token {
        Token::Start => return position == 0 && match_from(branch, token_index + 1, text, position),
        Token::End => return position == text.len() && match_from(branch, token_index + 1, text, position),
        _ => {}
    }
    match quant {
        Quant::One => {
            position < text.len()
                && token_matches(token, text[position])
                && match_from(branch, token_index + 1, text, position + 1)
        }
        Quant::Optional => {
            (position < text.len()
                && token_matches(token, text[position])
                && match_from(branch, token_index + 1, text, position + 1))
                || match_from(branch, token_index + 1, text, position)
        }
        Quant::ZeroOrMore | Quant::OneOrMore => {
            let minimum = if *quant == Quant::OneOrMore { 1 } else { 0 };
            let mut consumed = 0;
            while position + consumed < text.len() && token_matches(token, text[position + consumed]) {
                consumed += 1;
            }
            // 贪心回溯：先试最长，不行再退一个。`consumed + 1 > minimum`
            // 与下面的 `if consumed == 0 break` 共同保证循环必然收敛——
            // 少任何一个条件，最小匹配数为 0 时会在 consumed=0 处原地打转。
            loop {
                if consumed >= minimum && match_from(branch, token_index + 1, text, position + consumed) {
                    return true;
                }
                if consumed == 0 || consumed <= minimum {
                    return false;
                }
                consumed -= 1;
            }
        }
    }
}

/// 编译失败要明确报错，不能退回子串匹配——静默降级会让调用方以为正则生效了。
pub fn compile_lite_regex(pattern: &str) -> R<LiteRegex> {
    LiteRegex::new(pattern)
}

/// grep 风格的行级检索：返回命中行与行号，而不是整条记录。
///
/// `search` 回答"哪条记录含这个词"，grep 回答"哪一行"，是不同的问题。
/// 把两者混成一个工具会让返回结构随参数变化，调用方难以稳定解析。
pub fn grep(root: &str, pattern: &str, plane: Option<&str>, context: usize, regex: bool) -> R<Value> {
    if pattern.trim().is_empty() { return Err(err("memory_grep 需要非空 pattern")); }
    let root = root_path(root)?;
    let needle = pattern.to_lowercase();
    let matcher = if regex { Some(compile_lite_regex(&needle)?) } else { None };
    let matches = |line: &str| -> bool {
        let lower = line.to_lowercase();
        match &matcher {
            Some(program) => program.is_match(&lower),
            None => lower.contains(&needle),
        }
    };
    let mut hits = Vec::new();
    for item in records(&root, plane)? {
        let content = text(&item, "content").unwrap_or_default();
        let lines: Vec<&str> = content.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !matches(line) { continue; }
            let low = index.saturating_sub(context);
            let high = (index + context + 1).min(lines.len());
            let window: Vec<String> = (low..high)
                .map(|i| {
                    let marker = if i == index { ":" } else { "-" };
                    format!("{i + 1}{marker}{}", lines[i])
                })
                .collect();
            hits.push(object(vec![
                ("id", Value::s(text(&item, "id").unwrap_or_default())),
                ("plane", Value::s(text(&item, "plane").unwrap_or_default())),
                ("line", Value::n((index + 1).to_string())),
                ("lines", Value::arr(window.into_iter().map(Value::s).collect())),
            ]));
        }
    }
    let total = hits.len();
    Ok(object(vec![
        ("items", Value::Array(hits)),
        ("total", Value::n(total.to_string())),
        ("context", Value::n(context.to_string())),
    ]))
}

/// 合并两条记录：保留 primary，把 secondary 的内容并入后软删 secondary。
///
/// 规范把合并当常规维护，但 MCP 面此前只能"软删一条"，等于丢了一半信息。
/// 这里的合并策略是可解释的最小方案：
/// - 正文：primary 正文 + 分隔线 + secondary 正文（不智能去重，避免把
///   用户写下的两段相近但不同的判断悄悄消掉）；
/// - 空字段：用 secondary 补齐 primary 缺失的头部字段；
/// - 关系：两者的 relations 取并集去重；
/// - secondary 软删而不是物理删除，可回溯。
pub fn merge(
    root: &str,
    primary_id: &str,
    secondary_id: &str,
    title: Option<&str>,
    expected_primary: &str,
    expected_secondary: &str,
) -> R<Value> {
    if primary_id == secondary_id {
        return Err(err("不能把一条记录与自身合并"));
    }
    let root_path_buf = resolve_memory_path(root)?;
    let _lock = LibLock::acquire(&root_path_buf, 30)?;
    verify_layout(&root_path_buf)?;
    let primary = locate_record(&root_path_buf, primary_id)?.ok_or_else(|| err(format!("没有找到记忆记录: {primary_id}")))?;
    let secondary = locate_record(&root_path_buf, secondary_id)?.ok_or_else(|| err(format!("没有找到记忆记录: {secondary_id}")))?;
    if version_of(&primary.value) != expected_primary {
        return Err(err(format!("记忆 revision 冲突: expected={expected_primary} actual={}", version_of(&primary.value))));
    }
    if version_of(&secondary.value) != expected_secondary {
        return Err(err(format!("记忆 revision 冲突: expected={expected_secondary} actual={}", version_of(&secondary.value))));
    }
    if primary.legacy_raw.is_some() || secondary.legacy_raw.is_some() {
        return Err(err("旧迁移记录暂不支持合并：它们的字段编码与平面映射需要单独处理，强行合并会丢 legacy_ids"));
    }
    let primary_plane = text(&primary.value, "plane").ok_or_else(|| err("记忆记录缺少 plane"))?;
    let mut map = match primary.value.clone() {
        Value::Object(m) => m,
        _ => return Err(err("记忆记录必须是对象")),
    };

    let primary_body = text(&primary.value, "content").unwrap_or_default();
    let secondary_body = text(&secondary.value, "content").unwrap_or_default();
    let merged_body = if secondary_body.trim().is_empty() {
        primary_body
    } else if primary_body.trim().is_empty() {
        secondary_body
    } else {
        format!("{primary_body}\n\n---\n\n{secondary_body}")
    };
    map.insert("content".into(), Value::s(merged_body));

    if let Some(value) = title {
        map.insert("title".into(), Value::s(value));
    }

    // 补齐 primary 缺失的头部字段：不覆盖已有值，避免把主记录的信息冲掉。
    let fill_keys = [
        "read_when", "kind", "maturity", "authority", "summary", "brief", "not_for", "source",
    ];
    for key in fill_keys {
        let has = map.get(key).map(|v| !matches!(v, Value::Null) && text(&primary.value, key).map(|s| !s.trim().is_empty()).unwrap_or(false)).unwrap_or(false);
        if has {
            continue;
        }
        if let Some(value) = text(&secondary.value, key) {
            if !value.trim().is_empty() {
                map.insert(key.to_string(), Value::s(value));
            }
        }
    }

    // 关系取并集去重
    let mut relations: Vec<Value> = Vec::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for source in [&primary.value, &secondary.value] {
        if let Some(Value::Array(items)) = source.get("relations") {
            for item in items {
                let kind = text(item, "kind").unwrap_or_default();
                let target = text(item, "target").unwrap_or_default();
                if seen.insert((kind.clone(), target.clone())) {
                    relations.push(item.clone());
                }
            }
        }
    }
    map.insert("relations".into(), Value::Array(relations));

    let mut legacy_ids: Vec<Value> = Vec::new();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    for source in [&primary.value, &secondary.value] {
        for key in ["legacy_ids", "immutable"] {
            if let Some(Value::Array(items)) = source.get(key) {
                for item in items {
                    let id = match item {
                        Value::String(s) | Value::Number(s) => s.clone(),
                        _ => continue,
                    };
                    if seen_ids.insert(id.clone()) {
                        legacy_ids.push(Value::s(id));
                    }
                }
            }
        }
        let own = text(source, "id").unwrap_or_default();
        if seen_ids.insert(own.clone()) {
            legacy_ids.push(Value::s(own));
        }
    }
    map.insert("legacy_ids".into(), Value::Array(legacy_ids));
    map.insert(
        "merged_from".into(),
        Value::arr(vec![Value::s(primary_id), Value::s(secondary_id)]),
    );

    let next = version_of(&primary.value).parse::<u64>().unwrap_or(1).saturating_add(1);
    map.insert("revision".into(), Value::n(next.to_string()));
    map.insert("updated_at".into(), Value::n(now().to_string()));
    let merged = Value::Object(map);
    let primary_path = record_path(&root_path_buf, &primary_plane, primary_id)?;
    crate::util::write_atomic_cas(
        &primary_path,
        &mcp_json::stringify(&merged),
        &mcp_json::stringify(&primary.value),
    )?;

    // 副记录软删，可恢复。
    let secondary_plane = text(&secondary.value, "plane").ok_or_else(|| err("记忆记录缺少 plane"))?;
    let secondary_next = version_of(&secondary.value).parse::<u64>().unwrap_or(1).saturating_add(1);
    let mut secondary_map = match secondary.value.clone() {
        Value::Object(m) => m,
        _ => return Err(err("记忆记录必须是对象")),
    };
    secondary_map.insert("status".into(), Value::s("deleted"));
    secondary_map.insert("revision".into(), Value::n(secondary_next.to_string()));
    secondary_map.insert("updated_at".into(), Value::n(now().to_string()));
    secondary_map.insert("merged_into".into(), Value::s(primary_id));
    let secondary_path = record_path(&root_path_buf, &secondary_plane, secondary_id)?;
    crate::util::write_atomic_cas(
        &secondary_path,
        &mcp_json::stringify(&Value::Object(secondary_map)),
        &mcp_json::stringify(&secondary.value),
    )?;

    Ok(object(vec![
        ("id", Value::s(primary_id)),
        ("plane", Value::s(primary_plane)),
        ("revision", Value::n(next.to_string())),
        ("merged", Value::s(secondary_id)),
        ("status", Value::s("active")),
    ]))
}

/// 单条记录的变更历史。
///
/// 记录本身只保留最新状态（`revision` + `updated_at`），演化过程写在
/// 变更记录里。此前 MCP 面看不到某条是怎么变成现在这样的，而"这条为什么
/// 被改成这样"恰恰是复查时最先要问的问题。
pub fn history(root: &str, id: &str, limit: usize) -> R<Value> {
    if !valid_id(id) { return Err(err("非法 memory record id")); }
    let root_path_buf = resolve_memory_path(root)?;
    verify_layout(&root_path_buf)?;
    let located = locate_record(&root_path_buf, id)?
        .ok_or_else(|| err(format!("没有找到记忆记录: {id}")))?;
    let current = located.value.clone();
    let mut entries: Vec<Value> = Vec::new();

    // 记录自身的当前状态永远算一条。
    entries.push(object(vec![
        ("kind", Value::s("snapshot")),
        ("revision", Value::s(version_of(&current))),
        ("updated_at", Value::s(text(&current, "updated_at").unwrap_or_default())),
        ("status", Value::s(text(&current, "status").unwrap_or_else(|| "active".to_string()))),
        ("title", Value::s(text(&current, "title").unwrap_or_default())),
    ]));

    // 提交变更记录里凡是提到这个 ID 的都算历史线索。
    let commits = root_path_buf.join("commits");
    if commits.is_dir() {
        let mut files: Vec<_> = fs::read_dir(&commits)?.collect::<Result<Vec<_>, _>>()?;
        files.sort_by_key(|e| e.file_name());
        for entry in files {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let raw = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(_) => continue,
            };
            if !raw.contains(id) {
                continue;
            }
            let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let summary = raw
                .lines()
                .find(|line| line.to_lowercase().starts_with("summary="))
                .map(|line| line.splitn(2, '=').nth(1).unwrap_or("").trim().to_string())
                .unwrap_or_default();
            entries.push(object(vec![
                ("kind", Value::s("commit")),
                ("name", Value::s(name)),
                ("summary", Value::s(summary)),
            ]));
        }
    }

    entries.reverse();
    let total = entries.len();
    let size = limit.clamp(1, 500);
    entries.truncate(size);
    Ok(object(vec![
        ("id", Value::s(id)),
        ("items", Value::Array(entries)),
        ("total", Value::n(total.to_string())),
        ("limit", Value::n(size.to_string())),
    ]))
}

/// 回收站：明确只列软删记录。
///
/// 与 `list_filtered(status=deleted)` 等价，但单独给一个入口是因为
/// "我删过什么"是一个稳定的用户问题，不该要求调用方记住过滤参数怎么拼。
pub fn trash(root: &str, plane: Option<&str>, limit: usize, offset: usize) -> R<Value> {
    let root_path_buf = resolve_memory_path(root)?;
    verify_layout(&root_path_buf)?;
    let selected = plane.filter(|p| *p != "all");
    if let Some(name) = selected {
        if !plane_ok(name) { return Err(err(format!("未知记忆平面: {name}"))); }
    }
    let mut items = records_with_status(&root_path_buf, selected, true)?;
    items.retain(|item| text(item, "status").as_deref() == Some("deleted"));
    items.sort_by_key(|item| text(item, "id").unwrap_or_default());
    let total = items.len();
    let start = offset.min(total);
    let size = limit.clamp(1, 500);
    let page: Vec<Value> = items.into_iter().skip(start).take(size).collect();
    Ok(object(vec![
        ("items", Value::Array(page)),
        ("total", Value::n(total.to_string())),
        ("offset", Value::n(start.to_string())),
        ("limit", Value::n(size.to_string())),
    ]))
}
