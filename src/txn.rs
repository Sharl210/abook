//! 批次事务内核。
//!
//! 本模块故意不在 `main` 中接线：调用方负责决定何时取得库锁、如何选择目标，
//! 本模块只负责预备、暂存、校验、恢复与回滚的磁盘协议。所有落盘写入都复用
//! `util::write_atomic`，哈希复用现有的 `crate::sha256`，不引入第三方依赖。

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::sha256;
use crate::util::{read_str, EE, R};

/// 库级单调递增版本文件名。
pub const LIBRARY_REVISION: &str = "LIBRARY_REVISION";
/// 默认 NDJSON journal 文件名。
pub const JOURNAL_FILE: &str = ".txn.journal.ndjson";
/// 暂存文件目录名。
pub const STAGE_DIR: &str = ".txn/stage";
/// 预览 token 的协议前缀。
pub const PREVIEW_TOKEN_PREFIX: &str = "pt1";
/// 修订标记中记录事务身份，避免跨事务误认 revision 已递增。
pub const REVISION_TXN: &str = "LIBRARY_REVISION_TXN";

static NEXT_TRANSACTION_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

/// 事务阶段。阶段只向前推进，恢复和回滚是明确的终态分支。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Prepared,
    Staged,
    Applying,
    Committed,
    RecoveryRequired,
    RollingBack,
    RolledBack,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Prepared => "PREPARED",
            Phase::Staged => "STAGED",
            Phase::Applying => "APPLYING",
            Phase::Committed => "COMMITTED",
            Phase::RecoveryRequired => "RECOVERY_REQUIRED",
            Phase::RollingBack => "ROLLING_BACK",
            Phase::RolledBack => "ROLLED_BACK",
        }
    }

    fn parse(s: &str) -> R<Phase> {
        match s {
            "PREPARED" => Ok(Phase::Prepared),
            "STAGED" => Ok(Phase::Staged),
            "APPLYING" => Ok(Phase::Applying),
            "COMMITTED" => Ok(Phase::Committed),
            "RECOVERY_REQUIRED" => Ok(Phase::RecoveryRequired),
            "ROLLING_BACK" => Ok(Phase::RollingBack),
            "ROLLED_BACK" => Ok(Phase::RolledBack),
            _ => Err(EE::Integrity(format!("事务 journal 含未知阶段: {s}"))),
        }
    }
}

/// 一个相对于库根（或由调用方直接提供的绝对路径）的目标选择器。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selector {
    pub path: PathBuf,
}

impl Selector {
    pub fn new(path: impl Into<PathBuf>) -> Selector {
        Selector { path: path.into() }
    }

    fn display_path(&self) -> String {
        self.path.to_string_lossy().replace('\\', "/")
    }

    fn resolve(&self, root: &Path) -> R<PathBuf> {
        if self.path.as_os_str().is_empty() || self.path.is_absolute() {
            return Err(EE::Integrity(format!("事务 selector 必须是库根内相对路径: {}", self.display_path())));
        }
        let mut clean = PathBuf::new();
        for component in self.path.components() {
            match component {
                Component::Normal(part) => clean.push(part),
                Component::CurDir => {},
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(EE::Integrity(format!("事务 selector 越出库根或包含非法路径: {}", self.display_path())));
                }
            }
        }
        if clean.as_os_str().is_empty() { return Err(EE::Integrity("事务 selector 不能为空".into())); }
        Ok(root.join(clean))
    }
}

/// 已暂存的一项写操作。
///
/// `before_stage` 为空表示目标在预备时不存在；此时回滚会删除目标文件。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Operation {
    pub selector: Selector,
    pub stage_path: PathBuf,
    pub before_stage: Option<PathBuf>,
    pub before_hash: String,
    pub after_hash: String,
}

impl Operation {
    pub fn new(
        selector: Selector,
        stage_path: impl Into<PathBuf>,
        before_stage: Option<PathBuf>,
        before_hash: impl Into<String>,
        after_hash: impl Into<String>,
    ) -> Operation {
        Operation {
            selector,
            stage_path: stage_path.into(),
            before_stage,
            before_hash: before_hash.into(),
            after_hash: after_hash.into(),
        }
    }

    fn path(&self, root: &Path) -> R<PathBuf> {
        checked_internal_path(root, &self.selector.resolve(root)?)
    }
}

/// 一个 journal 行。字段保持扁平，便于无第三方 JSON 库的 NDJSON 往返。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalEntry {
    pub transaction: String,
    pub revision: u64,
    pub token: String,
    pub phase: Phase,
    /// `-1` 表示事务级阶段记录；非负数表示操作序号。
    pub index: i64,
    pub path: String,
    pub stage: String,
    pub before_stage: String,
    pub before_hash: String,
    pub after_hash: String,
}

impl JournalEntry {
    pub fn to_ndjson(&self) -> String {
        format!(
            "{{\"transaction\":\"{}\",\"revision\":{},\"token\":\"{}\",\"phase\":\"{}\",\"index\":{},\"path\":\"{}\",\"stage\":\"{}\",\"before_stage\":\"{}\",\"before_hash\":\"{}\",\"after_hash\":\"{}\"}}",
            json_escape(&self.transaction),
            self.revision,
            json_escape(&self.token),
            self.phase.as_str(),
            self.index,
            json_escape(&self.path),
            json_escape(&self.stage),
            json_escape(&self.before_stage),
            json_escape(&self.before_hash),
            json_escape(&self.after_hash),
        )
    }

    pub fn from_ndjson(line: &str) -> R<JournalEntry> {
        let fields = parse_json_object(line)?;
        let required = |name: &str| {
            fields
                .get(name)
                .cloned()
                .ok_or_else(|| EE::Integrity(format!("事务 journal 缺少字段: {name}")))
        };
        Ok(JournalEntry {
            transaction: required("transaction")?,
            revision: required("revision")?.parse().map_err(|_| {
                EE::Integrity("事务 journal 的 revision 不是非负整数".to_string())
            })?,
            token: required("token")?,
            phase: Phase::parse(&required("phase")?)?,
            index: required("index")?.parse().map_err(|_| {
                EE::Integrity("事务 journal 的 index 不是整数".to_string())
            })?,
            path: required("path")?,
            stage: required("stage")?,
            before_stage: required("before_stage")?,
            before_hash: required("before_hash")?,
            after_hash: required("after_hash")?,
        })
    }
}

/// 通过 `write_atomic` 维护的 NDJSON journal。
#[derive(Clone, Debug)]
pub struct Journal {
    pub path: PathBuf,
    pub entries: Vec<JournalEntry>,
}

impl Journal {
    pub fn at(path: impl Into<PathBuf>) -> Journal {
        Journal { path: path.into(), entries: Vec::new() }
    }

    pub fn load(path: impl Into<PathBuf>) -> R<Journal> {
        let path = path.into();
        if !path.exists() {
            return Ok(Journal::at(path));
        }
        let text = read_str(&path)?;
        let mut entries = Vec::new();
        for (line_no, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            entries.push(JournalEntry::from_ndjson(line).map_err(|e| {
                EE::Integrity(format!("解析事务 journal 第 {} 行失败: {}", line_no + 1, e.msg()))
            })?);
        }
        Ok(Journal { path, entries })
    }

    pub fn append(&mut self, entry: JournalEntry) -> R<()> {
        let mut next = String::new();
        for old in &self.entries {
            next.push_str(&old.to_ndjson());
            next.push('\n');
        }
        next.push_str(&entry.to_ndjson());
        next.push('\n');
        durable_write(&self.path, &next)?;
        self.entries.push(entry);
        Ok(())
    }
}

/// 事务对象。库锁不在这里取得，必须由调用方在整个生命周期外层持有。
pub struct Transaction {
    pub root: PathBuf,
    pub journal_path: PathBuf,
    pub transaction_id: String,
    pub revision: u64,
    pub phase: Phase,
    pub operations: Vec<Operation>,
    token: String,
    journal: Journal,
}

impl std::fmt::Debug for Transaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transaction")
            .field("root", &self.root)
            .field("journal_path", &self.journal_path)
            .field("transaction_id", &self.transaction_id)
            .field("revision", &self.revision)
            .field("phase", &self.phase)
            .field("operations", &self.operations)
            .field("token", &self.token)
            .finish()
    }
}

impl Transaction {
    /// 创建 PREPARED 事务并写入第一条 journal 行。
    pub fn prepare(root: impl AsRef<Path>) -> R<Transaction> {
        let root = absolute_root(root.as_ref())?;
        let revision = read_library_revision(&root)?;
        let serial = NEXT_TRANSACTION_ID.fetch_add(1, Ordering::Relaxed);
        let transaction_id = sha256::digest_hex(
            format!("txn\0{}\0{}\0{}\0{}", root.display(), revision, std::process::id(), serial)
                .as_bytes(),
        );
        let journal_path = root.join(JOURNAL_FILE);
        let journal = Journal::load(journal_path.clone())?;
        let existing = collect_logs(&journal.entries, &root)?;
        if let Some(active) = existing.values().find(|log| matches!(log.phase, Phase::Applying | Phase::RecoveryRequired | Phase::RollingBack)) {
            return Err(EE::Blocked(format!("库存在未完成事务 {}（{}），请先 recover", active.transaction, active.phase.as_str())));
        }
        let mut tx = Transaction {
            root,
            journal_path: journal_path.clone(),
            transaction_id,
            revision,
            phase: Phase::Prepared,
            operations: Vec::new(),
            token: String::new(),
            journal,
        };
        tx.token = tx.compute_preview_token();
        tx.record_phase(Phase::Prepared)?;
        Ok(tx)
    }

    pub fn preview_token(&self) -> String {
        self.compute_preview_token()
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// 将文本写入事务暂存区，同时保存 before 快照与 before/after hash。
    pub fn stage_file(&mut self, selector: Selector, content: &str) -> R<usize> {
        if self.phase != Phase::Prepared && self.phase != Phase::Staged {
            return Err(EE::Blocked(format!("事务处于 {}，不能继续暂存", self.phase.as_str())));
        }
        let path = checked_internal_path(&self.root, &selector.resolve(&self.root)?)?;
        for op in &self.operations {
            let op_path = op.path(&self.root)?;
            if op_path == path { return Err(EE::Integrity(format!("事务重复选择目标: {}", path.display()))); }
        }
        let before = read_optional_bytes(&path)?;
        let before_hash = hash_optional(before.as_deref());
        let index = self.operations.len();
        let stage_dir = self.root.join(STAGE_DIR);
        let stem = format!("{}-{index}", self.transaction_id);
        let stage_path = stage_dir.join(format!("{stem}.stage"));
        let before_stage = match before {
            Some(bytes) => {
                let old = String::from_utf8(bytes).map_err(|_| {
                    EE::Integrity(format!("事务目标不是 UTF-8 文本，无法复用 write_atomic: {}", path.display()))
                })?;
                let path = stage_dir.join(format!("{stem}.before"));
                durable_write(&path, &old)?;
                Some(path)
            }
            None => None,
        };
        durable_write(&stage_path, content)?;
        let op = Operation::new(
            selector,
            stage_path,
            before_stage,
            before_hash,
            sha256::digest_hex(content.as_bytes()),
        );
        self.operations.push(op.clone());
        let old_token = self.token.clone();
        self.token = self.compute_preview_token();
        let entry = self.operation_entry(Phase::Staged, index as i64, &op);
        if let Err(e) = self.journal.append(entry) {
            self.operations.pop();
            self.token = old_token;
            return Err(e);
        }
        self.phase = Phase::Staged;
        Ok(index)
    }

    /// 校验 preview token、before hash 后应用全部暂存文件。
    pub fn apply(&mut self, token: &str) -> R<u64> {
        if self.phase != Phase::Staged {
            return Err(EE::Blocked(format!("事务处于 {}，不能 apply", self.phase.as_str())));
        }
        if token != self.token {
            return Err(EE::Integrity("preview token 不匹配，拒绝 apply".to_string()));
        }
        self.verify_before_hashes()?;
        if let Err(e) = self.record_phase(Phase::Applying) {
            return Err(e);
        }
        for op in self.operations.clone() {
            let data = match read_str(&op.stage_path) {
                Ok(s) if sha256::digest_hex(s.as_bytes()) == op.after_hash => s,
                Ok(_) => {
                    return self.recovery_error(EE::Integrity(format!(
                        "暂存文件 hash 冲突，拒绝写入 {}",
                        op.path(&self.root)?.display()
                    )))
                }
                Err(e) => return self.recovery_error(e),
            };
            if let Err(e) = durable_write(&op.path(&self.root)?, &data) {
                return self.recovery_error(e);
            }
            match hash_file(&op.path(&self.root)?) {
                Ok(now) if now == op.after_hash => {}
                Ok(now) => {
                    return self.recovery_error(EE::Integrity(format!(
                        "应用后 hash 不符: {}，期望 {}/实际 {}",
                        op.path(&self.root)?.display(), op.after_hash, now
                    )))
                }
                Err(e) => return self.recovery_error(e),
            }
        }
        let next = match advance_library_revision(&self.root, self.revision, &self.transaction_id) {
            Ok(n) => n,
            Err(e) => return self.recovery_error(e),
        };
        if let Err(e) = self.record_phase(Phase::Committed) {
            return self.recovery_error(e);
        }
        Ok(next)
    }

    fn verify_before_hashes(&self) -> R<()> {
        for op in &self.operations {
            let now = hash_file(&op.path(&self.root)?)?;
            if now != op.before_hash {
                return Err(EE::Integrity(format!(
                    "hash 冲突，拒绝覆盖 {}：期望 before={}，实际={}",
                    op.path(&self.root)?.display(), op.before_hash, now
                )));
            }
        }
        Ok(())
    }

    fn recovery_error<T>(&mut self, error: EE) -> R<T> {
        let _ = self.record_phase(Phase::RecoveryRequired);
        Err(error)
    }

    fn compute_preview_token(&self) -> String {
        let mut material = Vec::new();
        material.extend_from_slice(PREVIEW_TOKEN_PREFIX.as_bytes());
        material.push(b'\n');
        material.extend_from_slice(&self.revision.to_be_bytes());
        material.extend_from_slice(&(self.operations.len() as u64).to_be_bytes());
        for (index, op) in self.operations.iter().enumerate() {
            material.extend_from_slice(&(index as u64).to_be_bytes());
            append_token_field(&mut material, op.selector.display_path().as_bytes());
            append_token_field(&mut material, op.before_hash.as_bytes());
            append_token_field(&mut material, op.after_hash.as_bytes());
        }
        format!("{PREVIEW_TOKEN_PREFIX}{}", sha256::digest_hex(&material))
    }

    fn operation_entry(&self, phase: Phase, index: i64, op: &Operation) -> JournalEntry {
        JournalEntry {
            transaction: self.transaction_id.clone(),
            revision: self.revision,
            token: self.token.clone(),
            phase,
            index,
            path: op.selector.display_path(),
            stage: op.stage_path.to_string_lossy().to_string(),
            before_stage: op
                .before_stage
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            before_hash: op.before_hash.clone(),
            after_hash: op.after_hash.clone(),
        }
    }

    fn record_phase(&mut self, phase: Phase) -> R<()> {
        let entry = JournalEntry {
            transaction: self.transaction_id.clone(),
            revision: self.revision,
            token: self.token.clone(),
            phase,
            index: -1,
            path: String::new(),
            stage: String::new(),
            before_stage: String::new(),
            before_hash: String::new(),
            after_hash: String::new(),
        };
        self.journal.append(entry)?;
        self.phase = phase;
        Ok(())
    }
}

fn append_token_field(out: &mut Vec<u8>, field: &[u8]) {
    out.extend_from_slice(&(field.len() as u64).to_be_bytes());
    out.extend_from_slice(field);
}

fn absolute_root(root: &Path) -> R<PathBuf> {
    let path = if root.is_absolute() { root.to_path_buf() } else { std::env::current_dir().map_err(EE::from)?.join(root) };
    if path.exists() { fs::canonicalize(&path).map_err(EE::from) } else { Ok(path) }
}

fn checked_internal_path(root: &Path, path: &Path) -> R<PathBuf> {
    let root = absolute_root(root)?;
    let candidate = if path.is_absolute() { path.to_path_buf() } else { root.join(path) };
    if !candidate.starts_with(&root) { return Err(EE::Integrity(format!("事务路径越出库根: {}", candidate.display()))); }
    let root_real = fs::canonicalize(&root).unwrap_or(root.clone());
    let check = if candidate.exists() { fs::canonicalize(&candidate).map_err(EE::from)? } else if let Some(parent) = candidate.parent() { if parent.exists() { fs::canonicalize(parent).map_err(EE::from)?.join(candidate.file_name().unwrap_or_default()) } else { candidate.clone() } } else { candidate.clone() };
    if !check.starts_with(&root_real) { return Err(EE::Integrity(format!("事务路径通过链接越出库根: {}", candidate.display()))); }
    Ok(candidate)
}
fn durable_write(path: &Path, data: &str) -> R<()> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    let serial = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("txn-tmp-{}-{serial}", std::process::id()));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(EE::from)?;
    file.write_all(data.as_bytes()).map_err(EE::from)?;
    file.sync_all().map_err(EE::from)?;
    drop(file);
    fs::rename(&tmp, path).map_err(EE::from)?;
    if let Some(parent) = path.parent() {
        let dir = File::open(parent).map_err(EE::from)?;
        dir.sync_all().map_err(EE::from)?;
    }
    Ok(())
}


/// 读取库级修订号。文件不存在等同于 0；损坏内容是完整性错误。
pub fn read_library_revision(root: impl AsRef<Path>) -> R<u64> {
    let path = root.as_ref().join(LIBRARY_REVISION);
    if !path.exists() {
        return Ok(0);
    }
    let raw = read_str(&path)?;
    raw.trim().parse::<u64>().map_err(|_| {
        EE::Integrity(format!("{} 不是非负整数", path.display()))
    })
}

/// 无条件将库级修订号递增一次并复读校验。
pub fn increment_library_revision(root: impl AsRef<Path>) -> R<u64> {
    let root = absolute_root(root.as_ref())?;
    let current = read_library_revision(&root)?;
    let txid = format!("manual-{}", NEXT_TRANSACTION_ID.fetch_add(1, Ordering::Relaxed));
    advance_library_revision(&root, current, &txid)
}

fn validate_library_revision(root: &Path, expected: u64, transaction: &str) -> R<()> {
    let current = read_library_revision(root)?;
    let next = expected.checked_add(1).ok_or_else(|| EE::Integrity("库级修订号溢出".to_string()))?;
    let marker_path = root.join(REVISION_TXN);
    if current == next {
        let marker = read_str(&marker_path).unwrap_or_default();
        if marker.trim() == format!("{transaction}|{expected}|{next}") { return Ok(()); }
        return Err(EE::Integrity(format!("revision {next} 已由其他事务占用，当前事务 {transaction} 不能重复认领")));
    }
    if current != expected { return Err(EE::Integrity(format!("库级修订号冲突：事务基线 {expected}，磁盘为 {current}"))); }
    Ok(())
}

fn advance_library_revision(root: &Path, expected: u64, transaction: &str) -> R<u64> {
    validate_library_revision(root, expected, transaction)?;
    let next = expected.checked_add(1).ok_or_else(|| EE::Integrity("库级修订号溢出".to_string()))?;
    let marker_path = root.join(REVISION_TXN);
    durable_write(&marker_path, &format!("{transaction}|{expected}|{next}\n"))?;
    durable_write(&root.join(LIBRARY_REVISION), &format!("{next}\n"))?;
    if read_library_revision(root)? != next { return Err(EE::Integrity("库级修订号复读不一致".into())); }
    Ok(next)
}


/// 从 durable journal 重建一个 staged 事务并按 token 应用。
/// 这使 preview 与 apply 可以跨进程工作，且 apply 仍会逐目标重验 before hash。
pub fn apply_token(root: impl AsRef<Path>, token: &str) -> R<u64> {
    let root = absolute_root(root.as_ref())?;
    let journal_path = root.join(JOURNAL_FILE);
    let journal = Journal::load(journal_path.clone())?;
    let logs = collect_logs(&journal.entries, &root)?;
    let log = logs.values().find(|log| log.token == token && log.phase == Phase::Staged)
        .ok_or_else(|| EE::NotFound(format!("没有可应用的 staged preview token: {token}")))?;
    let mut operations = Vec::new();
    for op in log.operations.values() { operations.push(op.clone()); }
    let mut tx = Transaction {
        root,
        journal_path,
        transaction_id: log.transaction.clone(),
        revision: log.revision,
        phase: Phase::Staged,
        operations,
        token: log.token.clone(),
        journal,
    };
    tx.apply(token)
}
/// 返回完整 SHA-256；不存在文件返回 `-`，用于 create/delete 的 before 状态。
pub fn hash_file(path: &Path) -> R<String> {
    match fs::read(path) {
        Ok(bytes) => Ok(sha256::digest_hex(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("-".to_string()),
        Err(e) => Err(EE::Io(format!("读取 hash 失败 {}: {e}", path.display()))),
    }
}

fn read_optional_bytes(path: &Path) -> R<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(EE::Io(format!("读取事务目标失败 {}: {e}", path.display()))),
    }
}

fn hash_optional(bytes: Option<&[u8]>) -> String {
    bytes.map(sha256::digest_hex).unwrap_or_else(|| "-".to_string())
}

/// 恢复模式下的动作结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryAction {
    DryRun,
    Committed,
    RolledBack,
    RecoveryRequired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryReport {
    pub transaction: String,
    pub token: String,
    pub phase: Phase,
    pub applied: Vec<String>,
    pub pending: Vec<String>,
    pub conflicts: Vec<String>,
    pub action: RecoveryAction,
}

/// `apply=false` 是 dry-run，`apply=true` 才会写入并推进 journal。
pub fn recover(root: impl AsRef<Path>, apply: bool) -> R<Vec<RecoveryReport>> {
    let root = absolute_root(root.as_ref())?;
    let path = root.join(JOURNAL_FILE);
    let mut journal = Journal::load(path)?;
    let logs = collect_logs(&journal.entries, &root)?;
    let mut reports = Vec::new();
    for log in logs.into_values() {
        if log.phase == Phase::RollingBack {
            let mut report = classify_recovery(&root, &log)?;
            if !apply { report.action = if report.conflicts.is_empty() { RecoveryAction::DryRun } else { RecoveryAction::RecoveryRequired }; reports.push(report); continue; }
            if !report.conflicts.is_empty() { report.action = RecoveryAction::RecoveryRequired; reports.push(report); continue; }
            let mut failed = None;
            for op in log.operations.values() {
                let path = op.path(&root)?;
                let current = hash_file(&path)?;
                if current == op.before_hash { continue; }
                if current != op.after_hash { failed = Some(EE::Integrity(format!("回滚恢复发现外部修改: {}", path.display()))); break; }
                match &op.before_stage {
                    Some(snapshot) => { let old = read_str(snapshot)?; if let Err(e) = durable_write(&path, &old) { failed = Some(e); break; } }
                    None => match fs::remove_file(&path) { Ok(()) => {}, Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}, Err(e) => { failed = Some(EE::Io(e.to_string())); break; } },
                }
            }
            if let Some(error) = failed { report.conflicts.push(error.msg().to_string()); report.action = RecoveryAction::RecoveryRequired; reports.push(report); continue; }
            let expected = log.revision.checked_add(1).ok_or_else(|| EE::Integrity("回滚 revision 溢出".into()))?;
            let rollback_id = format!("{}:rollback", log.transaction);
            if let Err(error) = advance_library_revision(&root, expected, &rollback_id) { report.conflicts.push(error.msg().to_string()); report.action = RecoveryAction::RecoveryRequired; reports.push(report); continue; }
            append_log_phase(&mut journal, &log, Phase::RolledBack)?;
            report.phase = Phase::RolledBack; report.action = RecoveryAction::RolledBack; reports.push(report); continue;
        }
        if log.phase != Phase::Applying && log.phase != Phase::RecoveryRequired { continue; }
        let mut report = classify_recovery(&root, &log)?;
        if !apply { report.action = if report.conflicts.is_empty() { RecoveryAction::DryRun } else { RecoveryAction::RecoveryRequired }; reports.push(report); continue; }
        if !report.conflicts.is_empty() { append_log_phase(&mut journal, &log, Phase::RecoveryRequired)?; report.action = RecoveryAction::RecoveryRequired; reports.push(report); continue; }
        let mut failed = None;
        for op in log.operations.values() {
            let path = op.path(&root)?;
            if hash_file(&path)? == op.after_hash {
                continue;
            }
            let stage = match read_str(&op.stage_path) {
                Ok(data) if sha256::digest_hex(data.as_bytes()) == op.after_hash => data,
                Ok(_) => {
                    failed = Some(EE::Integrity(format!(
                        "恢复暂存文件 hash 冲突: {}", op.stage_path.display()
                    )));
                    break;
                }
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            };
            match durable_write(&path, &stage) {
                Ok(()) => {
                    if hash_file(&path)? != op.after_hash {
                        failed = Some(EE::Integrity(format!(
                            "恢复应用后 hash 不符: {}", path.display()
                        )));
                        break;
                    }
                }
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        if let Some(error) = failed {
            append_log_phase(&mut journal, &log, Phase::RecoveryRequired)?;
            report.action = RecoveryAction::RecoveryRequired;
            report.conflicts.push(error.msg().to_string());
            reports.push(report);
            continue;
        }
        if let Err(error) = advance_library_revision(&root, log.revision, &log.transaction) {
            append_log_phase(&mut journal, &log, Phase::RecoveryRequired)?;
            report.action = RecoveryAction::RecoveryRequired;
            report.conflicts.push(error.msg().to_string());
            reports.push(report);
            continue;
        }
        append_log_phase(&mut journal, &log, Phase::Committed)?;
        report.applied.extend(report.pending.drain(..));
        report.phase = Phase::Committed;
        report.action = RecoveryAction::Committed;
        reports.push(report);
    }
    Ok(reports)
}

pub fn recover_dry_run(root: impl AsRef<Path>) -> R<Vec<RecoveryReport>> {
    recover(root, false)
}

pub fn recover_apply(root: impl AsRef<Path>) -> R<Vec<RecoveryReport>> {
    recover(root, true)
}

#[derive(Clone, Debug)]
struct TransactionLog {
    transaction: String,
    revision: u64,
    token: String,
    phase: Phase,
    operations: BTreeMap<i64, Operation>,
}

fn collect_logs(entries: &[JournalEntry], root: &Path) -> R<BTreeMap<String, TransactionLog>> {
    let mut out = BTreeMap::new();
    for entry in entries {
        let log = out.entry(entry.transaction.clone()).or_insert_with(|| TransactionLog {
            transaction: entry.transaction.clone(),
            revision: entry.revision,
            token: entry.token.clone(),
            phase: entry.phase,
            operations: BTreeMap::new(),
        });
        log.phase = entry.phase;
        if !entry.token.is_empty() {
            log.token = entry.token.clone();
        }
        if entry.index >= 0 && !entry.stage.is_empty() {
            log.operations.insert(entry.index, operation_from_entry(entry, root)?);
        }
    }
    Ok(out)
}

fn operation_from_entry(entry: &JournalEntry, root: &Path) -> R<Operation> {
    let before_stage = if entry.before_stage.is_empty() { None } else { Some(checked_internal_path(root, Path::new(&entry.before_stage))?) };
    let stage_path = checked_internal_path(root, Path::new(&entry.stage))?;
    Ok(Operation::new(Selector::new(entry.path.as_str()), stage_path, before_stage, entry.before_hash.as_str(), entry.after_hash.as_str()))
}

fn classify_recovery(root: &Path, log: &TransactionLog) -> R<RecoveryReport> {
    let mut report = RecoveryReport {
        transaction: log.transaction.clone(),
        token: log.token.clone(),
        phase: log.phase,
        applied: Vec::new(),
        pending: Vec::new(),
        conflicts: Vec::new(),
        action: RecoveryAction::DryRun,
    };
    for op in log.operations.values() {
        let path = op.path(&root)?;
        let current = hash_file(&path)?;
        let display = path.to_string_lossy().to_string();
        if current == op.after_hash {
            report.applied.push(display);
        } else if current == op.before_hash {
            match hash_file(&op.stage_path)? {
                stage_hash if stage_hash == op.after_hash => report.pending.push(display),
                stage_hash => report.conflicts.push(format!("{}: 暂存文件 hash 期望={}，实际={}", op.stage_path.display(), op.after_hash, stage_hash)),
            }
        } else {
            report.conflicts.push(format!("{}: 期望 before={} 或 after={}，实际={}", path.display(), op.before_hash, op.after_hash, current));
        }
    }
    Ok(report)
}

fn append_log_phase(journal: &mut Journal, log: &TransactionLog, phase: Phase) -> R<()> {
    journal.append(JournalEntry {
        transaction: log.transaction.clone(),
        revision: log.revision,
        token: log.token.clone(),
        phase,
        index: -1,
        path: String::new(),
        stage: String::new(),
        before_stage: String::new(),
        before_hash: String::new(),
        after_hash: String::new(),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RollbackReport {
    pub transaction: String,
    pub restored: Vec<String>,
    pub already_before: Vec<String>,
}

/// 回滚指定 preview token 的事务。每个目标必须处于 after 或已经处于 before；
/// 任何第三种 hash 都会拒绝回滚，避免覆盖事务外部的新修改。
pub fn rollback(root: impl AsRef<Path>, token: &str) -> R<RollbackReport> {
    let root = absolute_root(root.as_ref())?;
    let mut journal = Journal::load(root.join(JOURNAL_FILE))?;
    let logs = collect_logs(&journal.entries, &root)?;
    let transaction = journal
        .entries
        .iter()
        .rev()
        .find(|entry| entry.token == token)
        .map(|entry| entry.transaction.as_str())
        .ok_or_else(|| EE::NotFound(format!("没有找到 preview token: {token}")))?;
    let log = logs
        .get(transaction)
        .ok_or_else(|| EE::Integrity("事务 journal 缺少 token 对应事务".to_string()))?;
    let mut restored = Vec::new();
    let mut already_before = Vec::new();
    for op in log.operations.values() {
        let path = op.path(&root)?;
        let current = hash_file(&path)?;
        if current != op.before_hash && current != op.after_hash {
            return Err(EE::Integrity(format!("回滚 hash 冲突，拒绝覆盖 {}：期望 before={} 或 after={}，实际={}", path.display(), op.before_hash, op.after_hash, current)));
        }
        if let Some(snapshot) = &op.before_stage {
            let old = read_str(snapshot)?;
            if sha256::digest_hex(old.as_bytes()) != op.before_hash { return Err(EE::Integrity(format!("回滚 before 快照 hash 不符: {}", snapshot.display()))); }
        }
    }
    let rollback_expected = log.revision.checked_add(1).ok_or_else(|| EE::Integrity("回滚 revision 溢出".into()))?;
    let rollback_id = format!("{}:rollback", log.transaction);
    // Validate the revision before changing either targets or journal state.
    // Keep the actual advance after restoration so ROLLING_BACK remains durable.
    validate_library_revision(&root, rollback_expected, &rollback_id)?;
    append_log_phase(&mut journal, log, Phase::RollingBack)?;
    for op in log.operations.values() {
        let path = op.path(&root)?;
        let current = hash_file(&path)?;
        if current == op.before_hash { already_before.push(path.to_string_lossy().to_string()); continue; }
        if current != op.after_hash { return Err(EE::Integrity(format!("回滚 hash 在预检后发生变化，拒绝覆盖 {}", path.display()))); }
        match &op.before_stage {
            Some(snapshot) => { let old = read_str(snapshot)?; durable_write(&path, &old)?; }
            None => match fs::remove_file(&path) { Ok(()) => {}, Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}, Err(e) => return Err(EE::Io(format!("删除回滚目标失败 {}: {e}", path.display()))) },
        }
        if hash_file(&path)? != op.before_hash { return Err(EE::Integrity(format!("回滚后 hash 不符: {}", path.display()))); }
        restored.push(path.to_string_lossy().to_string());
    }
    advance_library_revision(&root, rollback_expected, &rollback_id)?;
    append_log_phase(&mut journal, log, Phase::RolledBack)?;
    Ok(RollbackReport { transaction: log.transaction.clone(), restored, already_before })
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c <= '\u{1f}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(ch),
        }
    }
    out
}

fn parse_json_object(line: &str) -> R<BTreeMap<String, String>> {
    let bytes = line.as_bytes();
    let mut pos = 0usize;
    skip_ws(bytes, &mut pos);
    expect_byte(bytes, &mut pos, b'{')?;
    let mut fields = BTreeMap::new();
    loop {
        skip_ws(bytes, &mut pos);
        if pos < bytes.len() && bytes[pos] == b'}' {
            pos += 1;
            break;
        }
        let key = parse_json_string(bytes, &mut pos)?;
        skip_ws(bytes, &mut pos);
        expect_byte(bytes, &mut pos, b':')?;
        skip_ws(bytes, &mut pos);
        let value = if pos < bytes.len() && bytes[pos] == b'"' {
            parse_json_string(bytes, &mut pos)?
        } else {
            let start = pos;
            while pos < bytes.len() && bytes[pos] != b',' && bytes[pos] != b'}' {
                pos += 1;
            }
            String::from_utf8(bytes[start..pos].to_vec()).map_err(|_| {
                EE::Integrity("事务 journal 含非法 UTF-8 值".to_string())
            })?.trim().to_string()
        };
        fields.insert(key, value);
        skip_ws(bytes, &mut pos);
        if pos < bytes.len() && bytes[pos] == b',' {
            pos += 1;
            continue;
        }
        expect_byte(bytes, &mut pos, b'}')?;
        break;
    }
    skip_ws(bytes, &mut pos);
    if pos != bytes.len() {
        return Err(EE::Integrity("事务 journal 对象后存在多余字符".to_string()));
    }
    Ok(fields)
}

fn skip_ws(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() && bytes[*pos].is_ascii_whitespace() {
        *pos += 1;
    }
}

fn expect_byte(bytes: &[u8], pos: &mut usize, wanted: u8) -> R<()> {
    if *pos >= bytes.len() || bytes[*pos] != wanted {
        return Err(EE::Integrity(format!("事务 journal JSON 缺少字符 {:?}", wanted as char)));
    }
    *pos += 1;
    Ok(())
}

fn parse_json_string(bytes: &[u8], pos: &mut usize) -> R<String> {
    expect_byte(bytes, pos, b'"')?;
    let mut out = Vec::new();
    while *pos < bytes.len() {
        let b = bytes[*pos];
        *pos += 1;
        match b {
            b'"' => {
                return String::from_utf8(out)
                    .map_err(|_| EE::Integrity("事务 journal 字符串不是 UTF-8".to_string()));
            }
            b'\\' => {
                if *pos >= bytes.len() {
                    return Err(EE::Integrity("事务 journal 转义不完整".to_string()));
                }
                let escaped = bytes[*pos];
                *pos += 1;
                match escaped {
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    b'/' => out.push(b'/'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'u' => {
                        if *pos + 4 > bytes.len() {
                            return Err(EE::Integrity("事务 journal Unicode 转义不完整".to_string()));
                        }
                        let mut code = 0u32;
                        for _ in 0..4 {
                            code = (code << 4)
                                | (bytes[*pos] as char).to_digit(16).ok_or_else(|| {
                                    EE::Integrity("事务 journal Unicode 转义非法".to_string())
                                })?;
                            *pos += 1;
                        }
                        let ch = char::from_u32(code).ok_or_else(|| {
                            EE::Integrity("事务 journal Unicode 码点非法".to_string())
                        })?;
                        let mut encoded = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
                    }
                    _ => return Err(EE::Integrity("事务 journal 含未知转义".to_string())),
                }
            }
            0..=0x1f => return Err(EE::Integrity("事务 journal 字符串含控制符".to_string())),
            _ => out.push(b),
        }
    }
    Err(EE::Integrity("事务 journal 字符串未闭合".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::write_atomic;

    fn temp_root(label: &str) -> PathBuf {
        let id = NEXT_TRANSACTION_ID.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("abook-txn-{label}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preview_token_is_stable_for_same_revision_and_operations() {
        let a = temp_root("token-a");
        let b = temp_root("token-b");
        let mut tx_a = Transaction::prepare(&a).unwrap();
        let mut tx_b = Transaction::prepare(&b).unwrap();
        tx_a.stage_file(Selector::new("record.md"), "after").unwrap();
        tx_b.stage_file(Selector::new("record.md"), "after").unwrap();
        assert_eq!(tx_a.preview_token(), tx_b.preview_token());
        assert!(tx_a.preview_token().starts_with("pt1"));
        assert_eq!(tx_a.preview_token().len(), 3 + 64);
        cleanup(&a);
        cleanup(&b);
    }

    #[test]
    fn journal_round_trip_preserves_ndjson_entry() {
        let root = temp_root("journal");
        let path = root.join("journal.ndjson");
        let mut journal = Journal::at(&path);
        let entry = JournalEntry {
            transaction: "txn-中文".to_string(),
            revision: 7,
            token: "pt1deadbeef".to_string(),
            phase: Phase::Staged,
            index: 2,
            path: "records/a\"b.md".to_string(),
            stage: "/tmp/a.stage".to_string(),
            before_stage: String::new(),
            before_hash: "-".to_string(),
            after_hash: "abc".to_string(),
        };
        journal.append(entry.clone()).unwrap();
        let loaded = Journal::load(&path).unwrap();
        assert_eq!(loaded.entries, vec![entry]);
        cleanup(&root);
    }

    #[test]
    fn apply_refuses_hash_conflict_before_writing() {
        let root = temp_root("conflict");
        let path = root.join("record.md");
        write_atomic(&path, "before").unwrap();
        let mut tx = Transaction::prepare(&root).unwrap();
        tx.stage_file(Selector::new("record.md"), "after").unwrap();
        write_atomic(&path, "outside").unwrap();
        let token = tx.token().to_string();
        let err = tx.apply(&token).unwrap_err();
        assert!(matches!(err, EE::Integrity(_)));
        assert_eq!(read_str(&path).unwrap(), "outside");
        cleanup(&root);
    }

    #[test]
    fn rollback_refuses_when_after_hash_no_longer_matches() {
        let root = temp_root("rollback-conflict");
        let path = root.join("record.md");
        write_atomic(&path, "before").unwrap();
        let mut tx = Transaction::prepare(&root).unwrap();
        tx.stage_file(Selector::new("record.md"), "after").unwrap();
        let token = tx.token().to_string();
        tx.apply(&token).unwrap();
        write_atomic(&path, "outside").unwrap();
        let err = rollback(&root, &token).unwrap_err();
        assert!(matches!(err, EE::Integrity(_)));
        assert_eq!(read_str(&path).unwrap(), "outside");
        cleanup(&root);
    }

    #[test]
    fn recovery_dry_run_classifies_before_and_after() {
        let root = temp_root("recover");
        let mut tx = Transaction::prepare(&root).unwrap();
        tx.stage_file(Selector::new("record.md"), "after").unwrap();
        let op = tx.operations[0].clone();
        tx.journal
            .append(JournalEntry {
                transaction: tx.transaction_id.clone(),
                revision: tx.revision,
                token: tx.token.clone(),
                phase: Phase::Applying,
                index: -1,
                path: String::new(),
                stage: String::new(),
                before_stage: String::new(),
                before_hash: String::new(),
                after_hash: String::new(),
            })
            .unwrap();
        let reports = recover_dry_run(&root).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(
            reports[0].pending,
            vec![root.join("record.md").to_string_lossy().into_owned()]
        );
        assert!(reports[0].applied.is_empty());
        write_atomic(&op.path(&root).unwrap(), "after").unwrap();
        let reports = recover_dry_run(&root).unwrap();
        assert_eq!(
            reports[0].applied,
            vec![root.join("record.md").to_string_lossy().into_owned()]
        );
        assert!(reports[0].pending.is_empty());
        cleanup(&root);
    }

    #[test]
    fn selector_rejects_escape_and_absolute_paths() {
        let root = temp_root("path-boundary");
        assert!(Transaction::prepare(&root).unwrap().stage_file(Selector::new("../outside.md"), "x").is_err());
        assert!(Transaction::prepare(&root).unwrap().stage_file(Selector::new(root.join("outside.md")), "x").is_err());
        cleanup(&root);
    }

    #[test]
    fn recovery_of_rolling_back_continues_backward() {
        let root = temp_root("rollback-recover");
        let mut tx = Transaction::prepare(&root).unwrap();
        tx.stage_file(Selector::new("record.md"), "after").unwrap();
        let token = tx.token().to_string();
        tx.apply(&token).unwrap();
        let logs = collect_logs(&tx.journal.entries, &root).unwrap();
        let log = logs.get(&tx.transaction_id).unwrap().clone();
        let mut journal = Journal::load(&tx.journal_path).unwrap();
        append_log_phase(&mut journal, &log, Phase::RollingBack).unwrap();
        let reports = recover_apply(&root).unwrap();
        assert_eq!(reports[0].action, RecoveryAction::RolledBack);
        assert!(!root.join("record.md").exists());
        assert_eq!(read_library_revision(&root).unwrap(), 2);
        cleanup(&root);
    }
    #[test]
    fn multi_file_commit_and_rollback_restore_mixed_baseline() {
        let root = temp_root("multi-file-roundtrip");
        write_atomic(&root.join("existing.md"), "原文\n😀").unwrap();
        let mut tx = Transaction::prepare(&root).unwrap();
        tx.stage_file(Selector::new("existing.md"), "更新\n😀").unwrap();
        tx.stage_file(Selector::new("new/a.md"), "new A").unwrap();
        tx.stage_file(Selector::new("new/b.md"), "new B").unwrap();
        let token = tx.token().to_string();
        assert_eq!(tx.apply(&token).unwrap(), 1);
        assert_eq!(read_str(&root.join("existing.md")).unwrap(), "更新\n😀");
        assert_eq!(read_str(&root.join("new/a.md")).unwrap(), "new A");
        assert_eq!(read_str(&root.join("new/b.md")).unwrap(), "new B");
        let report = rollback(&root, &token).unwrap();
        assert_eq!(report.restored.len(), 3);
        assert_eq!(read_str(&root.join("existing.md")).unwrap(), "原文\n😀");
        assert!(!root.join("new/a.md").exists());
        assert!(!root.join("new/b.md").exists());
        assert_eq!(read_library_revision(&root).unwrap(), 2);
        cleanup(&root);
    }

    #[test]
    fn rollback_revision_conflict_has_no_side_effects() {
        let root = temp_root("rollback-revision-conflict-no-side-effect");
        let path = root.join("record.md");
        write_atomic(&path, "before").unwrap();
        let mut tx = Transaction::prepare(&root).unwrap();
        tx.stage_file(Selector::new("record.md"), "after").unwrap();
        let token = tx.token().to_string();
        tx.apply(&token).unwrap();
        let revision_before = read_library_revision(&root).unwrap();
        write_atomic(&root.join("LIBRARY_REVISION"), "99\n").unwrap();
        let journal_before = read_str(&root.join(JOURNAL_FILE)).unwrap();
        let err = rollback(&root, &token).unwrap_err();
        assert!(matches!(err, EE::Integrity(_)));
        assert_eq!(read_str(&path).unwrap(), "after");
        assert_eq!(read_str(&root.join("LIBRARY_REVISION")).unwrap(), "99\n");
        assert_eq!(read_str(&root.join(JOURNAL_FILE)).unwrap(), journal_before);
        assert_ne!(revision_before, 99);
        cleanup(&root);
    }


    #[test]
    fn rollback_advances_revision_after_forward_commit() {
        let root = temp_root("rollback-revision");
        let mut tx = Transaction::prepare(&root).unwrap();
        tx.stage_file(Selector::new("record.md"), "after").unwrap();
        let token = tx.token().to_string();
        assert_eq!(tx.apply(&token).unwrap(), 1);
        assert_eq!(read_library_revision(&root).unwrap(), 1);
        rollback(&root, &token).unwrap();
        assert_eq!(read_library_revision(&root).unwrap(), 2);
        assert!(!root.join("record.md").exists());
        cleanup(&root);
    }
}