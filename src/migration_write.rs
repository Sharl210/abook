//! Staged, write-only migration plan built from [`super::Manifest`].
//!
//! This module deliberately does not expose a CLI path and never writes the
//! source library.  A build is assembled below a temporary sibling of the
//! caller-provided staging root, verified, and only then renamed to that root.
//! The destination must not already exist and must not overlap the source root.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use super::{Manifest, ManifestEntry, TargetPlane};

use crate::rec::{self, Lib, Record};
use crate::sha256;
use crate::util::{EE, LibLock, R};

fn integrity_error(message: impl Into<String>) -> EE {
    match crate::util::integrity::<()>(message) { Err(error) => error, Ok(()) => unreachable!() }
}


const PLAN_DIR: &str = "planes";
const ARCHIVE_DIR: &str = "archive";
const ATTACHMENTS_JSONL: &str = "attachments.jsonl";
const ATTACHMENTS_TSV: &str = "attachments.tsv";

const MANIFEST_JSONL: &str = "manifest.jsonl";
const MANIFEST_TSV: &str = "manifest.tsv";
const ALIASES_JSONL: &str = "legacy_aliases.jsonl";
const ALIASES_TSV: &str = "legacy_aliases.tsv";
const REPORT_TXT: &str = "migration-report.txt";
const ARCHIVE_KINDS: [&str; 3] = ["vault", "commits", "review"];
const ALL_PLANES: [&str; 7] = [
    "soul",
    "rules",
    "profile",
    "explicit",
    "long_term",
    "episodic",
    "tombstones",
];

/// One copied record in the staged plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedRecord {
    pub source_rel: String,
    pub target_rel: String,
    pub target_plane: String,
    pub source_class: String,
    pub classification_basis: String,
    pub body_sha256: String,
    pub raw_sha256: String,
    pub body_bytes: usize,
    pub body_lines: usize,
    pub unknown_fields: Vec<String>,
    pub conflict: bool,
}

/// One old alias mapping.  There is one row per alias/target pair, so an
/// ambiguous alias is never collapsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedAlias {
    pub alias: String,
    pub target_rel: String,
    pub sources: Vec<String>,
}

/// A read-only inventory row for vault/commits/review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub category: String,
    pub source_rel: String,
    pub kind: String,
    pub bytes: u64,
    pub sha256: Option<String>,
}

/// One copied attachment in the staged plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedAttachment {
    pub source_rel: String,
    pub target_rel: String,
    pub kind: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug)]
pub struct StagedManifest {
    pub source_root: PathBuf,
    pub staging_root: PathBuf,
    pub records: Vec<StagedRecord>,
    pub aliases: Vec<StagedAlias>,
    pub archives: BTreeMap<String, Vec<ArchiveEntry>>,
    pub attachments: Vec<StagedAttachment>,
    pub readable_report: String,
}

/// Build a complete migration plan under an explicit temporary staging root.
///
/// The source `Manifest` and every source record are read only.  The requested
/// root must not exist already; this prevents an accidental overwrite and makes
/// the final rename the only visible commit point.  On any failure the private
/// partial directory is removed and the requested root is left untouched.
pub fn build_staged(manifest: &Manifest, staging_root: &Path) -> R<StagedManifest> {
    let source_root = canonical_dir(&manifest.root, "源库根目录")?;
    let final_root = validate_staging_root(&source_root, staging_root)?;

    if !manifest.unreadable.is_empty() {
        return Err(integrity_error(format!(
            "迁移存在 {} 条不可读旧记录，拒绝生成不完整 staging",
            manifest.unreadable.len()
        )));
    }

    let prepared = prepare_records(manifest, &source_root)?;
    let aliases = prepare_aliases(manifest, &prepared)?;
    let archives = prepare_archives(&source_root)?;

    let attachments = prepare_attachments(&prepared, &source_root)?;

    let partial = partial_path(&final_root)?;
    if path_exists_any(&partial) {
        return Err(integrity_error(format!(
            "staging 临时路径已存在，拒绝覆盖: {}",
            partial.display()
        )));
    }
    fs::create_dir(&partial)?;

    let result = (|| -> R<StagedManifest> {
        for plane in ALL_PLANES {
            fs::create_dir_all(partial.join(PLAN_DIR).join(plane).join("records"))?;
        }
        for category in ARCHIVE_KINDS { fs::create_dir_all(partial.join(ARCHIVE_DIR).join(category))?; }
        copy_archives(&source_root, &partial, &archives)?;
        copy_attachments(&source_root, &partial, &attachments)?;

        for item in &prepared {
            let target = partial.join(&item.staged.target_rel);
            write_new_file(&target, &item.raw)?;
        }

        let mut readable_report = manifest.render_report();
        readable_report.push_str("\nstaged root: ");
        readable_report.push_str(&final_root.display().to_string());
        readable_report.push_str("\nrecords copied: ");
        readable_report.push_str(&prepared.len().to_string());
        readable_report.push_str("\nsource files are preserved as read-only archive inputs; no source switch performed.\n");
        let staged = StagedManifest {
            source_root: source_root.clone(),
            staging_root: partial.clone(),
            records: prepared.iter().map(|p| p.staged.clone()).collect(),
            aliases: aliases.clone(),
            archives: archives.clone(),
            attachments: attachments.clone(),
            readable_report,
        };
        write_reports(manifest, &staged)?;
        verify_staged(&staged)?;
        Ok(staged)
    })();

    let staged = match result {
        Ok(value) => value,
        Err(err) => {
            let _ = fs::remove_dir_all(&partial);
            return Err(err);
        }
    };

    // The destination did not exist when validated, and no writer touches it
    // before this point.  Rename is therefore the sole commit/switch operation.
    if let Err(err) = fs::rename(&partial, &final_root) {
        let _ = fs::remove_dir_all(&partial);
        return Err(err.into());
    }

    let committed = StagedManifest {
        source_root,
        staging_root: final_root.clone(),
        records: staged.records,
        aliases: staged.aliases,
        archives: staged.archives,
        attachments: staged.attachments,
        readable_report: staged.readable_report,
    };
    // 目录已从 partial 改名为 final；若提交后复验失败，立即移除这个不可信的
    // staging 根，避免把“返回错误但产物仍像已提交”的状态留在磁盘。
    if let Err(error) = verify_staged(&committed) {
        let _ = fs::remove_dir_all(&final_root);
        return Err(integrity_error(format!("staging 改名后复验失败，已清理不可信目录: {:?}", error)));
    }
    Ok(committed)
}


#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationReceipt {
    pub id: String,
    pub live_root: PathBuf,
    pub staging_root: PathBuf,
    pub backup_root: PathBuf,
    pub old_tree_sha256: String,
    pub new_tree_sha256: String,
    pub phase: String,
}

fn activation_marker(parent: &Path, id: &str) -> PathBuf {
    parent.join(format!(".abook-migration-{id}.receipt"))
}

fn receipt_text(receipt: &ActivationReceipt) -> String {
    format!(
        "version=2\nid={}\nlive_root={}\nstaging_root={}\nbackup_root={}\nold_tree_sha256={}\nnew_tree_sha256={}\nphase={}\n",
        receipt.id,
        receipt.live_root.display(),
        receipt.staging_root.display(),
        receipt.backup_root.display(),
        receipt.old_tree_sha256,
        receipt.new_tree_sha256,
        receipt.phase,
    )
}

fn write_receipt(marker: &Path, receipt: &ActivationReceipt) -> R<()> {
    crate::util::write_atomic(marker, &receipt_text(receipt))?;
    if let Some(parent) = marker.parent() {
        OpenOptions::new().read(true).open(parent)?.sync_all()?;
    }
    Ok(())
}

fn read_receipt(marker: &Path) -> R<ActivationReceipt> {
    let raw = fs::read_to_string(marker)?;
    let mut values = BTreeMap::new();
    for line in raw.lines() {
        let (key, value) = line.split_once('=').ok_or_else(|| integrity_error("迁移 receipt 行格式无效"))?;
        values.insert(key, value.to_string());
    }
    let version = values.remove("version").unwrap_or_default();
    if version != "1" && version != "2" {
        return Err(integrity_error("迁移 receipt 版本不支持"));
    }
    let old_tree_sha256 = values.remove("old_tree_sha256").unwrap_or_default();
    let new_tree_sha256 = values.remove("new_tree_sha256").unwrap_or_default();
    if version == "2" && (old_tree_sha256.is_empty() || new_tree_sha256.is_empty()) {
        return Err(integrity_error("迁移 receipt v2 缺少目录摘要"));
    }
    Ok(ActivationReceipt {
        id: values.remove("id").ok_or_else(|| integrity_error("迁移 receipt 缺少 id"))?,
        live_root: PathBuf::from(values.remove("live_root").ok_or_else(|| integrity_error("迁移 receipt 缺少 live_root"))?),
        staging_root: PathBuf::from(values.remove("staging_root").ok_or_else(|| integrity_error("迁移 receipt 缺少 staging_root"))?),
        backup_root: PathBuf::from(values.remove("backup_root").ok_or_else(|| integrity_error("迁移 receipt 缺少 backup_root"))?),
        old_tree_sha256,
        new_tree_sha256,
        phase: values.remove("phase").ok_or_else(|| integrity_error("迁移 receipt 缺少 phase"))?,
    })
}

fn sync_parent(path: &Path) -> R<()> {
    let parent = path.parent().ok_or_else(|| integrity_error("目录没有父目录"))?;
    OpenOptions::new().read(true).open(parent)?.sync_all()?;
    Ok(())
}

fn verify_hash(path: &Path, expected: &str, label: &str) -> R<()> {
    let bytes = fs::read(path).map_err(|_| integrity_error(format!("缺少{label}: {}", path.display())))?;
    let actual = sha256::digest_hex(&bytes);
    if actual != expected {
        return Err(integrity_error(format!("{label} hash 不符: {}", path.display())));
    }
    Ok(())
}

fn tree_digest(root: &Path) -> R<String> {
    let root = canonical_dir(root, "迁移目录")?;
    let mut entries = Vec::new();
    fn visit(root: &Path, dir: &Path, entries: &mut Vec<String>) -> R<()> {
        let mut children = fs::read_dir(dir)?.collect::<std::result::Result<Vec<_>, _>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
            let path = entry.path();
            let rel = path.strip_prefix(root).map_err(|_| integrity_error("目录摘要相对路径计算失败"))?.to_string_lossy().replace('\\', "/");
            let meta = fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() { return Err(integrity_error(format!("目录摘要发现符号链接: {rel}"))); }
            if meta.is_dir() {
                entries.push(format!("D|{rel}\n"));
                visit(root, &path, entries)?;
            } else if meta.is_file() {
                let bytes = fs::read(&path)?;
                entries.push(format!("F|{rel}|{}\n", sha256::digest_hex(&bytes)));
            } else {
                return Err(integrity_error(format!("目录摘要发现未知文件类型: {rel}")));
            }
        }
        Ok(())
    }
    visit(&root, &root, &mut entries)?;
    entries.sort();
    Ok(sha256::digest_hex(entries.concat().as_bytes()))
}
fn verify_source_baseline(staged: &StagedManifest, source: &Path) -> R<()> {
    for record in &staged.records {
        verify_hash(&source.join(&record.source_rel), &record.raw_sha256, "源记录")?;
    }
    for attachment in &staged.attachments {
        verify_hash(&source.join(&attachment.source_rel), &attachment.sha256, "源附件")?;
    }
    for entries in staged.archives.values() {
        for entry in entries {
            if let Some(expected) = &entry.sha256 {
                verify_hash(&source.join(&entry.source_rel), expected, "源归档")?;
            }
        }
    }
    Ok(())
}

pub fn verify_activated_root(root: &Path, staged: &StagedManifest) -> R<()> {
    let root = canonical_dir(root, "激活后的库根目录")?;
    for plane in ALL_PLANES {
        let dir = root.join(PLAN_DIR).join(plane).join("records");
        let meta = fs::symlink_metadata(&dir).map_err(|_| integrity_error(format!("激活库缺少平面目录: {}", dir.display())))?;
        if !meta.is_dir() || meta.file_type().is_symlink() { return Err(integrity_error(format!("激活平面目录类型不符: {}", dir.display()))); }
    }
    for record in &staged.records {
        verify_hash(&root.join(&record.target_rel), &record.raw_sha256, "激活记录")?;
    }
    for attachment in &staged.attachments {
        verify_hash(&root.join(&attachment.target_rel), &attachment.sha256, "激活附件")?;
    }
    for (category, entries) in &staged.archives {
        for entry in entries {
            if let Some(expected) = &entry.sha256 {
                verify_hash(&root.join(ARCHIVE_DIR).join(category).join(&entry.source_rel), expected, "激活归档")?;
            }
        }
    }
    for file in [MANIFEST_JSONL, MANIFEST_TSV, ALIASES_JSONL, ALIASES_TSV, ATTACHMENTS_JSONL, ATTACHMENTS_TSV, REPORT_TXT] {
        let path = root.join(file);
        let meta = fs::symlink_metadata(&path).map_err(|_| integrity_error(format!("激活库缺少报告文件: {}", path.display())))?;
        if !meta.is_file() || meta.file_type().is_symlink() { return Err(integrity_error(format!("激活报告文件类型不符: {}", path.display()))); }
    }
    Ok(())
}

pub fn activate_staged(staged: &StagedManifest) -> R<ActivationReceipt> {
    let live = canonical_dir(&staged.source_root, "源库根目录")?;
    let stage = canonical_dir(&staged.staging_root, "staging 根目录")?;
    if paths_overlap(&live, &stage) || live.parent() != stage.parent() {
        return Err(integrity_error("激活要求 live 与 staging 为同一父目录下的非重叠目录"));
    }
    verify_staged(staged)?;
    let _lock = LibLock::acquire(&live, 30)?;
    let old_tree_sha256 = tree_digest(&live)?;
    let new_tree_sha256 = tree_digest(&stage)?;
    verify_source_baseline(staged, &live)?;
    let parent = live.parent().ok_or_else(|| integrity_error("源库根目录没有父目录"))?;
    let serial = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let id = sha256::digest_hex(format!("{}:{}:{}", live.display(), std::process::id(), serial).as_bytes())[..16].to_string();
    let backup = parent.join(format!(".{}.abook-migration-backup-{id}", live.file_name().unwrap_or_default().to_string_lossy()));
    if path_exists_any(&backup) { return Err(integrity_error(format!("迁移 backup 已存在: {}", backup.display()))); }
    let marker = activation_marker(parent, &id);
    let mut receipt = ActivationReceipt {
        id,
        live_root: live.clone(),
        staging_root: stage.clone(),
        backup_root: backup.clone(),
        old_tree_sha256,
        new_tree_sha256,
        phase: "PREPARED".into(),
    };
    write_receipt(&marker, &receipt)?;
    fs::rename(&live, &backup)?;
    sync_parent(&backup)?;
    receipt.phase = "SOURCE_MOVED".into(); write_receipt(&marker, &receipt)?;
    if let Err(error) = fs::rename(&stage, &live) {
        let rollback = fs::rename(&backup, &live);
        if rollback.is_ok() {
            let _ = sync_parent(&live);
            let _ = fs::remove_file(&marker);
            return Err(integrity_error(format!("staging 切换失败，已恢复源库: {error}")));
        }
        receipt.phase = "RECOVERY_REQUIRED".into();
        let _ = write_receipt(&marker, &receipt);
        return Err(integrity_error(format!("staging 切换失败且自动回滚失败，需要 migrate --recover: {error}")));
    }
    sync_parent(&live)?;
    receipt.phase = "LIVE_MOVED".into(); write_receipt(&marker, &receipt)?;
    let activation_verification = verify_activated_root(&live, staged).and_then(|_| {
        let actual = tree_digest(&live)?;
        if actual != receipt.new_tree_sha256 {
            return Err(integrity_error(format!("激活目录摘要不匹配: expected={} actual={}", receipt.new_tree_sha256, actual)));
        }
        Ok(())
    });
    if let Err(error) = activation_verification {
        receipt.phase = "ROLLING_BACK".into(); let _ = write_receipt(&marker, &receipt);
        let failed = parent.join(format!(".{}.abook-migration-failed-{}", live.file_name().unwrap_or_default().to_string_lossy(), receipt.id));
        let rollback_result = fs::rename(&live, &failed).and_then(|_| fs::rename(&backup, &live));
        if rollback_result.is_ok() { let _ = sync_parent(&live); let _ = fs::remove_file(&marker); }
        return Err(integrity_error(format!("激活后验证失败，{}: {:?}", if rollback_result.is_ok() { "已回滚源库" } else { "需要执行 migrate --recover" }, error)));
    }
    receipt.phase = "VERIFIED".into(); write_receipt(&marker, &receipt)?;
    fs::remove_file(&marker)?;
    sync_parent(&live)?;
    Ok(receipt)
}

pub fn rollback_activation(live_root: &Path, backup_root: &Path) -> R<PathBuf> {
    let live = canonical_dir(live_root, "当前库根目录")?;
    let backup = canonical_dir(backup_root, "迁移 backup 根目录")?;
    if live.parent() != backup.parent() || live == backup { return Err(integrity_error("rollback 要求 live 与 backup 同父目录且不能相同")); }
    let parent = live.parent().ok_or_else(|| integrity_error("当前库根目录没有父目录"))?;
    let _lock = LibLock::acquire(&live, 30)?;
    let old_tree_sha256 = tree_digest(&backup)?;
    let new_tree_sha256 = tree_digest(&live)?;
    let id = sha256::digest_hex(format!("rollback:{}:{}", live.display(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()).as_bytes())[..16].to_string();
    let abandoned = parent.join(format!(".{}.abook-migration-abandoned-{id}", live.file_name().unwrap_or_default().to_string_lossy()));
    let marker = activation_marker(parent, &id);
    let mut receipt = ActivationReceipt {
        id,
        live_root: live.clone(),
        staging_root: abandoned.clone(),
        backup_root: backup.clone(),
        old_tree_sha256,
        new_tree_sha256,
        phase: "ROLLBACK_PREPARED".into(),
    };
    write_receipt(&marker, &receipt)?;
    fs::rename(&live, &abandoned)?;
    sync_parent(&abandoned)?;
    receipt.phase = "ROLLBACK_SOURCE_MOVED".into();
    write_receipt(&marker, &receipt)?;
    if let Err(error) = fs::rename(&backup, &live) {
        let restored = fs::rename(&abandoned, &live);
        if restored.is_ok() {
            let _ = sync_parent(&live);
            receipt.phase = "ROLLBACK_RESTORED".into();
            let _ = write_receipt(&marker, &receipt);
            let _ = fs::remove_file(&marker);
            return Err(integrity_error(format!("rollback 切换失败，已恢复原 live: {error}")));
        }
        receipt.phase = "RECOVERY_REQUIRED".into();
        let _ = write_receipt(&marker, &receipt);
        return Err(integrity_error(format!("rollback 切换和原 live 恢复均失败，需要 migrate --recover: {error}")));
    }
    sync_parent(&live)?;
    drop(_lock);
    let _restored_live_lock = match LibLock::acquire(&live, 30) {
        Ok(lock) => lock,
        Err(error) => return Err(integrity_error(format!("rollback 已恢复旧库但获取库锁失败: {:?}", error))),
    };
    receipt.phase = "VERIFIED".into();
    write_receipt(&marker, &receipt)?;
    fs::remove_file(&marker)?;
    Ok(abandoned)
}

pub fn recover_activation(live_root: &Path) -> R<ActivationReceipt> {
    let live = absolutize(live_root)?;
    let parent = live.parent().ok_or_else(|| integrity_error("live 根目录没有父目录"))?;
    // The live directory may be absent after a crash. LibLock derives its
    // stable path from the live pathname, so it remains the same lock used
    // before the directory rename and serializes recovery with writers.
    let _lock = LibLock::acquire(&live, 30)?;
    let mut found = Vec::new();
    for entry in fs::read_dir(parent)? {
        let path = entry?.path();
        if path.file_name().and_then(|s| s.to_str()).map(|s| s.starts_with(".abook-migration-") && s.ends_with(".receipt")).unwrap_or(false) {
            let receipt = read_receipt(&path)?;
            if receipt.live_root == live { found.push((path, receipt)); }
        }
    }
    if found.len() != 1 { return Err(integrity_error(format!("找到 {} 个匹配迁移 receipt，无法安全恢复", found.len()))); }
    let (marker, mut receipt) = found.remove(0);
    let backup_exists = path_exists_any(&receipt.backup_root);
    let live_exists = path_exists_any(&receipt.live_root);
    if receipt.old_tree_sha256.is_empty() || receipt.new_tree_sha256.is_empty() {
        return Err(integrity_error("旧版 receipt 没有目录摘要，保留现场，需人工核对后恢复"));
    }
    if live_exists && !backup_exists && (receipt.phase == "PREPARED" || receipt.phase == "ROLLBACK_SOURCE_MOVED") {
        let live_digest = tree_digest(&receipt.live_root)?;
        if live_digest == receipt.old_tree_sha256 {
            receipt.phase = if receipt.phase == "PREPARED" { "RECOVERED_NOT_STARTED" } else { "RECOVERED_ROLLED_BACK" }.into();
            write_receipt(&marker, &receipt)?;
            fs::remove_file(marker)?;
            return Ok(receipt);
        }
        return Err(integrity_error("迁移 receipt 的 live 目录摘要不匹配，保留现场供人工恢复"));
    }
    if live_exists && backup_exists {
        let live_digest = tree_digest(&receipt.live_root)?;
        let backup_digest = tree_digest(&receipt.backup_root)?;
        if receipt.phase == "ROLLBACK_SOURCE_MOVED" && live_digest == receipt.old_tree_sha256 && backup_digest == receipt.old_tree_sha256 {
            receipt.phase = "RECOVERED_ROLLED_BACK".into();
            write_receipt(&marker, &receipt)?;
            fs::remove_file(marker)?;
            return Ok(receipt);
        }
        if live_digest == receipt.new_tree_sha256 && backup_digest == receipt.old_tree_sha256 {
            receipt.phase = "RECOVERED_COMMITTED".into();
            write_receipt(&marker, &receipt)?;
            fs::remove_file(marker)?;
            return Ok(receipt);
        }
        if live_digest == receipt.old_tree_sha256 && backup_digest == receipt.new_tree_sha256 {
            receipt.phase = "RECOVERED_ROLLED_BACK".into();
            write_receipt(&marker, &receipt)?;
            fs::remove_file(marker)?;
            return Ok(receipt);
        }
        return Err(integrity_error("迁移 live/backup 目录摘要均不匹配，保留现场供人工恢复"));
    }
    if !live_exists && backup_exists {
        let backup_digest = tree_digest(&receipt.backup_root)?;
        if backup_digest != receipt.old_tree_sha256 {
            return Err(integrity_error("迁移 backup 目录摘要不匹配，拒绝自动回滚"));
        }
        fs::rename(&receipt.backup_root, &receipt.live_root)?;
        sync_parent(&receipt.live_root)?;
        let restored_digest = tree_digest(&receipt.live_root)?;
        if restored_digest != receipt.old_tree_sha256 {
            receipt.phase = "RECOVERY_REQUIRED".into();
            let _ = write_receipt(&marker, &receipt);
            return Err(integrity_error("自动回滚后目录摘要不匹配，保留 receipt 供人工恢复"));
        }
        receipt.phase = "RECOVERED_ROLLED_BACK".into();
        write_receipt(&marker, &receipt)?;
        fs::remove_file(marker)?;
        return Ok(receipt);
    }
    Err(integrity_error("迁移 receipt 的 live/backup 状态无法安全判定，保留现场供人工恢复"))
}
/// Verify an already-built staged tree without modifying it.
pub fn verify_staged(staged: &StagedManifest) -> R<()> {
    let root = canonical_dir(&staged.staging_root, "staging 根目录")?;
    let source = canonical_dir(&staged.source_root, "源库根目录")?;
    if paths_overlap(&source, &root) {
        return Err(integrity_error(format!(
            "源库与 staging 路径重叠: source={} staging={}",
            source.display(),
            root.display()
        )));
    }
    let expected_archives = prepare_archives(&source)?;
    if expected_archives != staged.archives {
        return Err(integrity_error("vault/commits/review 只读归档清单与源目录不一致"));
    }
    verify_archive_copies(&root, &staged.archives)?;
    let expected_attachments = expected_attachments(staged)?;
    if expected_attachments != staged.attachments { return Err(integrity_error("附件清单与源记录目录不一致")); }
    verify_attachments(&root, &staged.attachments)?;

    for plane in ALL_PLANES {
        let dir = root.join(PLAN_DIR).join(plane).join("records");
        if !dir.is_dir() || fs::symlink_metadata(&dir)?.file_type().is_symlink() {
            return Err(integrity_error(format!("缺少 staging 记录目录: {}", dir.display())));
        }
    }

    let mut seen = BTreeSet::new();
    for record in &staged.records {
        validate_relative(&record.target_rel, "目标记录路径")?;
        if !seen.insert(record.target_rel.clone()) {
            return Err(integrity_error(format!("目标记录路径重复: {}", record.target_rel)));
        }
        let target = root.join(&record.target_rel);
        ensure_within(&root, &target, "目标记录路径")?;
        let metadata = fs::symlink_metadata(&target).map_err(|_| {
            integrity_error(format!("staging 缺少记录文件: {}", target.display()))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(integrity_error(format!("目标记录不是普通文件: {}", target.display())));
        }
        let raw = fs::read(&target)?;
        if sha256::digest_hex(&raw) != record.raw_sha256 {
            return Err(integrity_error(format!("记录原文 SHA256 不匹配: {}", record.target_rel)));
        }
        let parsed = Record::parse_str(
            std::str::from_utf8(&raw)
                .map_err(|_| integrity_error(format!("记录不是 UTF-8: {}", record.target_rel)))?,
            &target,
            lib_from_target(&record.target_plane),
        )?;
        if body_digest(&parsed.body) != record.body_sha256 {
            return Err(integrity_error(format!("记录正文 SHA256 不匹配: {}", record.target_rel)));
        }
        let unknown = unknown_fields(&parsed);
        if unknown != record.unknown_fields {
            return Err(integrity_error(format!("未知字段清单不匹配: {}", record.target_rel)));
        }
    }

    let expected_manifest_jsonl = render_manifest_jsonl(&staged.records);
    let expected_manifest_tsv = render_manifest_tsv(&staged.records);
    check_text(&root.join(MANIFEST_JSONL), &expected_manifest_jsonl)?;
    check_text(&root.join(MANIFEST_TSV), &expected_manifest_tsv)?;

    let expected_aliases_jsonl = render_aliases_jsonl(&staged.aliases);
    let expected_aliases_tsv = render_aliases_tsv(&staged.aliases);
    check_text(&root.join(ALIASES_JSONL), &expected_aliases_jsonl)?;
    check_text(&root.join(ALIASES_TSV), &expected_aliases_tsv)?;

    for category in ARCHIVE_KINDS {
        let entries = staged.archives.get(category).cloned().unwrap_or_default();
        check_text(
            &root.join(ARCHIVE_DIR).join(format!("{category}.jsonl")),
            &render_archive_jsonl(&entries),
        )?;
        check_text(
            &root.join(ARCHIVE_DIR).join(format!("{category}.tsv")),
            &render_archive_tsv(&entries),
        )?;
    }

    check_text(&root.join(ATTACHMENTS_JSONL), &render_attachments_jsonl(&staged.attachments))?;
    check_text(&root.join(ATTACHMENTS_TSV), &render_attachments_tsv(&staged.attachments))?;
    check_text(&root.join(REPORT_TXT), &staged.readable_report)?;
    Ok(())
}

struct PreparedRecord {
    source: PathBuf,
    staged: StagedRecord,
    raw: Vec<u8>,
}

fn prepare_records(manifest: &Manifest, source_root: &Path) -> R<Vec<PreparedRecord>> {
    let mut indexes: Vec<usize> = (0..manifest.entries.len()).collect();
    indexes.sort_by(|a, b| {
        let left = &manifest.entries[*a];
        let right = &manifest.entries[*b];
        left.target_plane_label()
            .cmp(right.target_plane_label())
            .then_with(|| left.key.cmp(&right.key))
            .then_with(|| left.path_rel.cmp(&right.path_rel))
    });

    let mut out = Vec::with_capacity(indexes.len());
    let mut target_names = BTreeSet::new();
    for index in indexes {
        let entry = &manifest.entries[index];
        let source = canonical_file(&entry.path, "记录源路径")?;
        ensure_within(source_root, &source, "记录源路径")?;
        let source_rel = source
            .strip_prefix(source_root)
            .map_err(|_| integrity_error("记录源路径无法转成源库相对路径"))?
            .to_string_lossy()
            .replace('\\', "/");
        validate_relative(&source_rel, "源记录相对路径")?;
        let raw = fs::read(&source)?;
        let text = std::str::from_utf8(&raw)
            .map_err(|_| integrity_error(format!("记录不是 UTF-8: {}", source.display())))?;
        let parsed = Record::parse_str(text, &source, entry.key.lib)?;
        if parsed.lib != entry.key.lib || record_stem(&parsed) != entry.key.stem {
            return Err(integrity_error(format!(
                "manifest 与源记录身份不一致: {}",
                source.display()
            )));
        }
        let body_sha = body_digest(&parsed.body);
        if body_sha != entry.body_sha256 {
            return Err(integrity_error(format!(
                "manifest 与源记录正文 SHA256 不一致: {}",
                source.display()
            )));
        }
        let plane = plane_dir(entry.target_plane);
        let digest_input = format!(
            "{}\0{}\0{}\0{}",
            entry.key, source_rel, body_sha, sha256::digest_hex(&raw)
        );
        let target_name = format!("{}-{}.md", entry.key.lib.name(), sha256::digest_hex(digest_input.as_bytes()));
        let target_rel = format!("{PLAN_DIR}/{plane}/records/{target_name}");
        if !target_names.insert(target_rel.clone()) {
            return Err(integrity_error(format!("稳定目标文件名冲突: {target_rel}")));
        }
        let staged = StagedRecord {
            source_rel,
            target_rel,
            target_plane: entry.target_plane_label().to_string(),
            source_class: entry.source_class_label().to_string(),
            classification_basis: entry.target_basis_label(),
            body_sha256: body_sha,
            raw_sha256: sha256::digest_hex(&raw),
            body_bytes: entry.body_bytes,
            body_lines: entry.body_lines,
            unknown_fields: unknown_fields(&parsed),
            conflict: entry_has_conflict(manifest, entry),
        };
        out.push(PreparedRecord { source, staged, raw });
    }
    Ok(out)
}

fn prepare_aliases(manifest: &Manifest, records: &[PreparedRecord]) -> R<Vec<StagedAlias>> {
    let by_key: BTreeMap<_, _> = records
        .iter()
        .map(|record| (record_key_from_staged(record), record.staged.target_rel.clone()))
        .collect();
    let mut out = Vec::new();
    for alias in manifest.old_id_map.aliases() {
        let mut targets = manifest.old_id_map.targets(alias).to_vec();
        targets.sort_by(|a, b| a.key.cmp(&b.key));
        for target in targets {
            let target_rel = by_key.get(&target.key).ok_or_else(|| {
                integrity_error(format!("别名指向未生成的记录: {alias} -> {}", target.key))
            })?;
            let mut sources: Vec<String> = target.sources.iter().map(|s| s.as_str().to_string()).collect();
            sources.sort();
            out.push(StagedAlias {
                alias: alias.clone(),
                target_rel: target_rel.clone(),
                sources,
            });
        }
    }
    Ok(out)
}

fn prepare_archives(source_root: &Path) -> R<BTreeMap<String, Vec<ArchiveEntry>>> {
    let mut all = BTreeMap::new();
    for category in ARCHIVE_KINDS {
        let dir = source_root.join(category);
        let mut rows = Vec::new();
        if dir.exists() {
            if !dir.is_dir() || fs::symlink_metadata(&dir)?.file_type().is_symlink() {
                return Err(integrity_error(format!("归档源目录不是普通目录: {}", dir.display())));
            }
            inventory_archive(source_root, category, &dir, &mut rows)?;
        } else {
            rows.push(ArchiveEntry {
                category: category.to_string(),
                source_rel: category.to_string(),
                kind: "directory-missing".to_string(),
                bytes: 0,
                sha256: None,
            });
        }
        rows.sort_by(|a, b| a.source_rel.cmp(&b.source_rel).then(a.kind.cmp(&b.kind)));
        all.insert(category.to_string(), rows);
    }
    Ok(all)
}

fn copy_archives(source_root: &Path, staging_root: &Path, archives: &BTreeMap<String, Vec<ArchiveEntry>>) -> R<()> {
    for category in ARCHIVE_KINDS {
        let source_dir = source_root.join(category);
        if !source_dir.is_dir() { continue; }
        for entry in archives.get(category).into_iter().flatten() {
            let rel = entry.source_rel.strip_prefix(&format!("{category}/")).unwrap_or("");
            if rel.is_empty() { continue; }
            let source = source_root.join(&entry.source_rel);
            let target = staging_root.join(ARCHIVE_DIR).join(category).join(rel);
            if entry.kind == "directory" { fs::create_dir_all(&target)?; }
            else if entry.kind == "file" {
                let bytes = fs::read(&source)?;
                if entry.sha256.as_deref() != Some(sha256::digest_hex(&bytes).as_str()) { return Err(integrity_error(format!("归档源文件 hash 在复制前变化: {}", source.display()))); }
                write_new_file(&target, &bytes)?;
            }
        }
    }
    Ok(())
}
fn verify_archive_copies(staging_root: &Path, archives: &BTreeMap<String, Vec<ArchiveEntry>>) -> R<()> {
    for category in ARCHIVE_KINDS {
        let base = staging_root.join(ARCHIVE_DIR).join(category);
        for entry in archives.get(category).into_iter().flatten() {
            let rel = entry.source_rel.strip_prefix(&format!("{category}/")).unwrap_or("");
            if rel.is_empty() || entry.kind == "directory-missing" { continue; }
            let target = base.join(rel);
            ensure_within(staging_root, &target, "staged archive path")?;
            match entry.kind.as_str() {
                "directory" => {
                    let meta = fs::symlink_metadata(&target).map_err(|_| integrity_error(format!("staged archive 缺目录: {}", target.display())))?;
                    if !meta.is_dir() || meta.file_type().is_symlink() { return Err(integrity_error(format!("staged archive 目录类型不符: {}", target.display()))); }
                }
                "file" => {
                    let meta = fs::symlink_metadata(&target).map_err(|_| integrity_error(format!("staged archive 缺文件: {}", target.display())))?;
                    if !meta.is_file() || meta.file_type().is_symlink() { return Err(integrity_error(format!("staged archive 文件类型不符: {}", target.display()))); }
                    let bytes = fs::read(&target)?;
                    let digest = sha256::digest_hex(&bytes);
                    if bytes.len() as u64 != entry.bytes || entry.sha256.as_deref() != Some(digest.as_str()) { return Err(integrity_error(format!("staged archive 内容指纹不符: {}", entry.source_rel))); }
                }
                "symlink" => if path_exists_any(&target) { return Err(integrity_error(format!("staged archive 不应复制符号链接: {}", entry.source_rel))); },
                _ => return Err(integrity_error(format!("未知 archive 类型 {}: {}", entry.kind, entry.source_rel))),
            }
        }
    }
    Ok(())
}


fn inventory_archive(source_root: &Path, category: &str, dir: &Path, out: &mut Vec<ArchiveEntry>) -> R<()> {
    let mut children = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let path = child.path();
        ensure_within(source_root, &path, "归档源路径")?;
        let rel = path
            .strip_prefix(source_root)
            .map_err(|_| integrity_error("归档源路径无法转成相对路径"))?
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = fs::symlink_metadata(&path)?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            out.push(ArchiveEntry {
                category: category.to_string(),
                source_rel: rel,
                kind: "symlink".to_string(),
                bytes: 0,
                sha256: None,
            });
        } else if metadata.is_dir() {
            out.push(ArchiveEntry {
                category: category.to_string(),
                source_rel: rel.clone(),
                kind: "directory".to_string(),
                bytes: 0,
                sha256: None,
            });
            inventory_archive(source_root, category, &path, out)?;
        } else if metadata.is_file() {
            let bytes = fs::read(&path)?;
            out.push(ArchiveEntry {
                category: category.to_string(),
                source_rel: rel,
                kind: "file".to_string(),
                bytes: bytes.len() as u64,
                sha256: Some(sha256::digest_hex(&bytes)),
            });
        }
    }
    Ok(())
}

fn write_reports(manifest: &Manifest, staged: &StagedManifest) -> R<()> {
    let root = &staged.staging_root;
    write_new_file(&root.join(MANIFEST_JSONL), render_manifest_jsonl(&staged.records).as_bytes())?;
    write_new_file(&root.join(MANIFEST_TSV), render_manifest_tsv(&staged.records).as_bytes())?;
    write_new_file(&root.join(ALIASES_JSONL), render_aliases_jsonl(&staged.aliases).as_bytes())?;
    write_new_file(&root.join(ALIASES_TSV), render_aliases_tsv(&staged.aliases).as_bytes())?;
    write_new_file(&root.join(ATTACHMENTS_JSONL), render_attachments_jsonl(&staged.attachments).as_bytes())?;
    write_new_file(&root.join(ATTACHMENTS_TSV), render_attachments_tsv(&staged.attachments).as_bytes())?;
    for category in ARCHIVE_KINDS {
        let entries = staged.archives.get(category).cloned().unwrap_or_default();
        write_new_file(
            &root.join(ARCHIVE_DIR).join(format!("{category}.jsonl")),
            render_archive_jsonl(&entries).as_bytes(),
        )?;
        write_new_file(
            &root.join(ARCHIVE_DIR).join(format!("{category}.tsv")),
            render_archive_tsv(&entries).as_bytes(),
        )?;
    }
    if !staged.readable_report.starts_with(&manifest.render_report()) { return Err(integrity_error("staged 人类可读报告与 manifest 基线不一致")); }
    write_new_file(&root.join(REPORT_TXT), staged.readable_report.as_bytes())?;

    Ok(())
}

fn render_attachments_jsonl(items: &[StagedAttachment]) -> String {
    items.iter().map(|item| format!("{{\"source_rel\":{},\"target_rel\":{},\"kind\":{},\"bytes\":{},\"sha256\":{}}}", json_str(&item.source_rel), json_str(&item.target_rel), json_str(&item.kind), item.bytes, json_str(&item.sha256))).collect::<Vec<_>>().join("\n") + if items.is_empty() { "" } else { "\n" }
}

fn render_attachments_tsv(items: &[StagedAttachment]) -> String {
    let mut out = String::from("source_rel\ttarget_rel\tkind\tbytes\tsha256\n");
    for item in items { out.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", tsv(&item.source_rel), tsv(&item.target_rel), tsv(&item.kind), item.bytes, item.sha256)); }
    out
}

fn render_manifest_jsonl(records: &[StagedRecord]) -> String {
    records.iter().map(record_json).collect::<Vec<_>>().join("\n") + if records.is_empty() { "" } else { "\n" }
}

fn record_json(record: &StagedRecord) -> String {
    format!(
        "{{\"source_rel\":{},\"target_rel\":{},\"target_plane\":{},\"source_class\":{},\"classification_basis\":{},\"body_sha256\":{},\"raw_sha256\":{},\"body_bytes\":{},\"body_lines\":{},\"unknown_fields\":{},\"conflict\":{}}}",
        json_str(&record.source_rel),
        json_str(&record.target_rel),
        json_str(&record.target_plane),
        json_str(&record.source_class),
        json_str(&record.classification_basis),
        json_str(&record.body_sha256),
        json_str(&record.raw_sha256),
        record.body_bytes,
        record.body_lines,
        json_array(&record.unknown_fields),
        record.conflict,
    )
}

fn render_manifest_tsv(records: &[StagedRecord]) -> String {
    let mut out = String::from("source_rel\ttarget_rel\ttarget_plane\tsource_class\tclassification_basis\tbody_sha256\traw_sha256\tbody_bytes\tbody_lines\tunknown_fields\tconflict\n");
    for record in records {
        let unknown = record.unknown_fields.join(",");
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            tsv(&record.source_rel),
            tsv(&record.target_rel),
            tsv(&record.target_plane),
            tsv(&record.source_class),
            tsv(&record.classification_basis),
            record.body_sha256,
            record.raw_sha256,
            record.body_bytes,
            record.body_lines,
            tsv(&unknown),
            record.conflict,
        ));
    }
    out
}

fn render_aliases_jsonl(aliases: &[StagedAlias]) -> String {
    aliases.iter().map(|a| {
        format!("{{\"alias\":{},\"target_rel\":{},\"sources\":{}}}", json_str(&a.alias), json_str(&a.target_rel), json_array(&a.sources))
    }).collect::<Vec<_>>().join("\n") + if aliases.is_empty() { "" } else { "\n" }
}

fn render_aliases_tsv(aliases: &[StagedAlias]) -> String {
    let mut out = String::from("alias\ttarget_rel\tsources\n");
    for alias in aliases {
        out.push_str(&format!("{}\t{}\t{}\n", tsv(&alias.alias), tsv(&alias.target_rel), tsv(&alias.sources.join(","))));
    }
    out
}

fn render_archive_jsonl(entries: &[ArchiveEntry]) -> String {
    entries.iter().map(|e| {
        format!("{{\"category\":{},\"source_rel\":{},\"kind\":{},\"bytes\":{},\"sha256\":{}}}", json_str(&e.category), json_str(&e.source_rel), json_str(&e.kind), e.bytes, e.sha256.as_deref().map(json_str).unwrap_or_else(|| "null".to_string()))
    }).collect::<Vec<_>>().join("\n") + if entries.is_empty() { "" } else { "\n" }
}

fn render_archive_tsv(entries: &[ArchiveEntry]) -> String {
    let mut out = String::from("category\tsource_rel\tkind\tbytes\tsha256\n");
    for entry in entries {
        out.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", tsv(&entry.category), tsv(&entry.source_rel), tsv(&entry.kind), entry.bytes, entry.sha256.as_deref().unwrap_or("")));
    }
    out
}

fn json_str(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_array(values: &[String]) -> String {
    format!("[{}]", values.iter().map(|v| json_str(v)).collect::<Vec<_>>().join(","))
}

fn tsv(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\t', "\\t").replace('\r', "\\r").replace('\n', "\\n")
}

fn check_text(path: &Path, expected: &str) -> R<()> {
    let actual = fs::read_to_string(path).map_err(|_| integrity_error(format!("缺少报告文件: {}", path.display())))?;
    if actual != expected {
        return Err(integrity_error(format!("报告内容被修改: {}", path.display())));
    }
    Ok(())
}

fn write_new_file(path: &Path, bytes: &[u8]) -> R<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn validate_staging_root(source_root: &Path, requested: &Path) -> R<PathBuf> {
    if requested.as_os_str().is_empty() {
        return Err(integrity_error("staging root 不能为空，必须显式传入临时目录"));
    }
    let absolute = absolutize(requested)?;
    if path_exists_any(&absolute) {
        return Err(integrity_error(format!("staging root 已存在，拒绝覆盖: {}", absolute.display())));
    }
    let parent = absolute.parent().ok_or_else(|| integrity_error("staging root 没有父目录"))?;
    let parent = canonical_dir(parent, "staging 父目录")?;
    let name = absolute.file_name().ok_or_else(|| integrity_error("staging root 名称为空"))?;
    let final_root = parent.join(name);
    if paths_overlap(source_root, &final_root) {
        return Err(integrity_error(format!(
            "staging root 与源库重叠，拒绝写入: {}",
            final_root.display()
        )));
    }
    Ok(final_root)
}

fn partial_path(final_root: &Path) -> R<PathBuf> {
    let name = final_root.file_name().ok_or_else(|| integrity_error("staging root 名称为空"))?.to_string_lossy();
    Ok(final_root.parent().unwrap_or_else(|| Path::new(".")).join(format!(".{name}.abook-staging-partial")))
}

fn canonical_dir(path: &Path, label: &str) -> R<PathBuf> {
    let p = fs::canonicalize(path).map_err(|_| integrity_error(format!("{label} 不存在或不可解析: {}", path.display())))?;
    let md = fs::symlink_metadata(&p)?;
    if !md.is_dir() || md.file_type().is_symlink() {
        return Err(integrity_error(format!("{label} 不是普通目录: {}", path.display())));
    }
    Ok(p)
}

fn canonical_file(path: &Path, label: &str) -> R<PathBuf> {
    let p = fs::canonicalize(path).map_err(|_| integrity_error(format!("{label} 不存在或不可解析: {}", path.display())))?;
    let md = fs::symlink_metadata(&p)?;
    if !md.is_file() || md.file_type().is_symlink() {
        return Err(integrity_error(format!("{label} 不是普通文件: {}", path.display())));
    }
    Ok(p)
}

fn absolutize(path: &Path) -> R<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn path_exists_any(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn paths_overlap(a: &Path, b: &Path) -> bool {
    a == b || a.starts_with(b) || b.starts_with(a)
}

fn prepare_attachments(prepared: &[PreparedRecord], source_root: &Path) -> R<Vec<StagedAttachment>> {
    let mut out = Vec::new();
    for record in prepared { collect_record_attachments(source_root, &record.source, &record.staged.target_rel, &mut out)?; }
    out.sort_by(|a, b| a.target_rel.cmp(&b.target_rel));
    if out.windows(2).any(|pair| pair[0].target_rel == pair[1].target_rel) { return Err(integrity_error("附件目标路径重复")); }
    Ok(out)
}

fn expected_attachments(staging: &StagedManifest) -> R<Vec<StagedAttachment>> {
    let mut out = Vec::new();
    for record in &staging.records {
        let source = staging.source_root.join(&record.source_rel);
        collect_record_attachments(&staging.source_root, &source, &record.target_rel, &mut out)?;
    }
    out.sort_by(|a, b| a.target_rel.cmp(&b.target_rel));
    Ok(out)
}

fn collect_record_attachments(source_root: &Path, source_record: &Path, target_record_rel: &str, out: &mut Vec<StagedAttachment>) -> R<()> {
    let target_path = Path::new(target_record_rel);
    let parent = target_path.parent().ok_or_else(|| integrity_error("目标记录缺父目录"))?;
    let target_stem = target_path.file_stem().ok_or_else(|| integrity_error("目标记录缺 stem"))?.to_string_lossy();
    for dir in crate::attach::all_dirs(source_record) {
        let dir_name = dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "refs".to_string());
        let suffix = dir_name.rsplit_once('-').map(|(_, suffix)| suffix).unwrap_or("refs");
        let target_base = format!("{}/{target_stem}-{suffix}", parent.to_string_lossy().replace('\\', "/"));
        let source_rel = dir.strip_prefix(source_root).map_err(|_| integrity_error("附件目录源路径越界"))?.to_string_lossy().replace('\\', "/");
        out.push(StagedAttachment { source_rel, target_rel: target_base.clone(), kind: "directory".to_string(), bytes: 0, sha256: String::new() });
        collect_attachment_files(source_root, &dir, &dir, &target_base, out)?;
    }
    Ok(())
}

fn collect_attachment_files(source_root: &Path, base: &Path, current: &Path, target_base: &str, out: &mut Vec<StagedAttachment>) -> R<()> {
    let mut children = fs::read_dir(current)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let path = child.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() { return Err(integrity_error(format!("附件含符号链接，拒绝迁移: {}", path.display()))); }
        if meta.is_dir() {
            let rel = path.strip_prefix(base).map_err(|_| integrity_error("附件目录相对路径解析失败"))?.to_string_lossy().replace('\\', "/");
            validate_relative(&rel, "附件目录相对路径")?;
            let source_rel = path.strip_prefix(source_root).map_err(|_| integrity_error("附件目录源路径越界"))?.to_string_lossy().replace('\\', "/");
            let target_rel = format!("{target_base}/{rel}");
            out.push(StagedAttachment { source_rel, target_rel, kind: "directory".to_string(), bytes: 0, sha256: String::new() });
            collect_attachment_files(source_root, base, &path, target_base, out)?;
            continue;
        }
        if !meta.is_file() { return Err(integrity_error(format!("附件不是普通文件: {}", path.display()))); }
        let rel = path.strip_prefix(base).map_err(|_| integrity_error("附件相对路径解析失败"))?.to_string_lossy().replace('\\', "/");
        validate_relative(&rel, "附件相对路径")?;
        let bytes = fs::read(&path)?;
        let source_rel = path.strip_prefix(source_root).map_err(|_| integrity_error("附件源路径越界"))?.to_string_lossy().replace('\\', "/");
        let target_rel = format!("{target_base}/{rel}");
        validate_relative(&target_rel, "附件目标路径")?;
        out.push(StagedAttachment { source_rel, target_rel, kind: "file".to_string(), bytes: bytes.len() as u64, sha256: sha256::digest_hex(&bytes) });
    }
    Ok(())
}

fn copy_attachments(source_root: &Path, staging_root: &Path, attachments: &[StagedAttachment]) -> R<()> {
    for item in attachments {
        let source = source_root.join(&item.source_rel);
        let target = staging_root.join(&item.target_rel);
        ensure_within(source_root, &source, "附件源路径")?;
        ensure_within(staging_root, &target, "附件目标路径")?;
        if item.kind == "directory" { fs::create_dir_all(&target)?; continue; }
        let bytes = fs::read(&source)?;
        if bytes.len() as u64 != item.bytes || sha256::digest_hex(&bytes) != item.sha256 { return Err(integrity_error(format!("附件源在复制前发生漂移: {}", source.display()))); }
        write_new_file(&target, &bytes)?;
    }
    Ok(())
}

fn verify_attachments(staging_root: &Path, attachments: &[StagedAttachment]) -> R<()> {
    for item in attachments {
        let target = staging_root.join(&item.target_rel);
        ensure_within(staging_root, &target, "附件目标路径")?;
        if item.kind == "directory" {
            let meta = fs::symlink_metadata(&target).map_err(|_| integrity_error(format!("staging 缺少附件目录: {}", target.display())))?;
            if !meta.is_dir() || meta.file_type().is_symlink() { return Err(integrity_error(format!("staging 附件目录类型不符: {}", target.display()))); }
            continue;
        }
        let meta = fs::symlink_metadata(&target).map_err(|_| integrity_error(format!("staging 缺少附件: {}", target.display())))?;
        if !meta.is_file() || meta.file_type().is_symlink() { return Err(integrity_error(format!("staging 附件类型不符: {}", target.display()))); }
        let bytes = fs::read(&target)?;
        if bytes.len() as u64 != item.bytes || sha256::digest_hex(&bytes) != item.sha256 { return Err(integrity_error(format!("staging 附件 hash 不符: {}", item.target_rel))); }
    }
    Ok(())
}
fn ensure_within(root: &Path, child: &Path, label: &str) -> R<()> {
    if child.strip_prefix(root).is_err() {
        return Err(integrity_error(format!("{label} 越出边界: {}", child.display())));
    }
    Ok(())
}

fn validate_relative(path: &str, label: &str) -> R<()> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return Err(integrity_error(format!("{label} 不是安全相对路径: {path}")));
    }
    for component in Path::new(path).components() {
        match component {
            Component::Normal(_) => {}
            _ => return Err(integrity_error(format!("{label} 含非法路径组件: {path}"))),
        }
    }
    Ok(())
}

fn plane_dir(plane: TargetPlane) -> &'static str {
    match plane {
        TargetPlane::Soul => "soul",
        TargetPlane::Rules => "rules",
        TargetPlane::Profile => "profile",
        TargetPlane::Explicit => "explicit",
        TargetPlane::LongTerm => "long_term",
        TargetPlane::Episodic => "episodic",
        TargetPlane::Tombstone => "tombstones",
    }
}

fn lib_from_target(plane: &str) -> Lib {
    // The parser only uses this as a hint.  Both source libraries use the same
    // structural format, and the staged tree intentionally has no old lib tag.
    if plane == "tombstone" { Lib::Candidate } else { Lib::Canonical }
}

fn record_stem(record: &Record) -> String {
    let name = record.file_name();
    name.strip_suffix(".md").unwrap_or(&name).to_string()
}

fn record_key_from_staged(record: &PreparedRecord) -> super::RecordKey {
    let stem = Path::new(&record.staged.source_rel)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let lib = if record.staged.source_rel.starts_with("candidate/") {
        Lib::Candidate
    } else {
        Lib::Canonical
    };
    super::RecordKey { lib, stem }
}

fn protected_library_overlap(path: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    let Ok(home) = fs::canonicalize(home) else {
        return false;
    };
    let protected = home.join("AgentsBook");
    path == protected || path.starts_with(&protected)
}

fn unknown_fields(record: &Record) -> Vec<String> {
    let mut fields: Vec<String> = record
        .fields
        .iter()
        .map(|(key, _)| key.clone())
        .filter(|key| !rec::is_known_key(key))
        .collect();
    fields.sort();
    fields.dedup();
    fields
}

fn body_digest(body: &str) -> String {
    sha256::digest_hex(body.replace("\r\n", "\n").trim().as_bytes())
}

fn entry_has_conflict(manifest: &Manifest, entry: &ManifestEntry) -> bool {
    manifest.conflicts.body_sha_divergences.iter().any(|d| d.canonical == entry.key || d.candidate == entry.key)
        || manifest.conflicts.duplicate_body_sha.iter().any(|d| d.records.contains(&entry.key))
        || manifest.conflicts.duplicate_promoted_from.iter().any(|d| d.claimed_by.contains(&entry.key))
        || manifest.conflicts.ambiguous_aliases.iter().any(|a| a.targets.iter().any(|t| t.key == entry.key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("abook-migration-write-{tag}-{}-{n}", std::process::id()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn record(lib: Lib, stem: &str, unknown: &str, body: &str) -> String {
        let tag = lib.tag();
        format!("[{tag}]\nID={stem}\nTITLE=测试\nKIND=DECISION\n{unknown}\n[/{tag}]\n[MEMORY_BODY]\n{body}\n[/MEMORY_BODY]\n")
    }

    #[test]
    fn staged_build_is_stable_and_preserves_raw_unknown_and_reports() {
        let root = temp_dir("fixture");
        fs::create_dir_all(root.join("canonical/records")).unwrap();
        fs::create_dir_all(root.join("candidate/records")).unwrap();
        let source = root.join("canonical/records/C-20261101T120000-aaaaaaaa.md");
        let raw = record(Lib::Canonical, "C-20261101T120000-aaaaaaaa", "X_UNKNOWN=保留字段", "正文\n第二行");
        fs::write(&source, raw.as_bytes()).unwrap();
        let source_refs = root.join("canonical/records/C-20261101T120000-aaaaaaaa-refs");
        fs::create_dir_all(&source_refs).unwrap();
        fs::write(source_refs.join("proof.bin"), [0u8, 1, 2, 255]).unwrap();
        fs::create_dir_all(root.join("commits")).unwrap();
        fs::write(root.join("commits/historical.md"), "read-only history").unwrap();
        let manifest = migration::scan(&root).unwrap();
        let staging = root.parent().unwrap().join(format!("{}-out", root.file_name().unwrap().to_string_lossy()));
        let staged = build_staged(&manifest, &staging).unwrap();
        verify_staged(&staged).unwrap();
        assert!(staging.join("planes/long_term/records").is_dir());
        assert!(staging.join("manifest.jsonl").is_file());
        assert!(staging.join("manifest.tsv").is_file());
        assert!(staging.join("legacy_aliases.jsonl").is_file());
        assert!(staging.join("archive/vault.jsonl").is_file());
        assert_eq!(fs::read(staging.join("archive/commits/historical.md")).unwrap(), b"read-only history");
        assert_eq!(staged.records.len(), 1);
        let target = staging.join(&staged.records[0].target_rel);
        assert_eq!(fs::read(&target).unwrap(), raw.as_bytes());
        assert_eq!(staged.records[0].unknown_fields, vec!["X_UNKNOWN"]);
        let attachment = staged.attachments.iter().find(|item| item.kind == "file").unwrap();
        assert_eq!(fs::read(staging.join(&attachment.target_rel)).unwrap(), [0u8, 1, 2, 255]);
        assert_eq!(attachment.source_rel, "canonical/records/C-20261101T120000-aaaaaaaa-refs/proof.bin");
        let target_record = staging.join(&staged.records[0].target_rel);
        assert!(target_record.with_file_name(format!("{}-refs", target_record.file_stem().unwrap().to_string_lossy())).is_dir());
        let second = staging.with_file_name(format!("{}-2", staging.file_name().unwrap().to_string_lossy()));
        let staged2 = build_staged(&manifest, &second).unwrap();
        assert_eq!(staged.records[0].target_rel, staged2.records[0].target_rel);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(staging);
        let _ = fs::remove_dir_all(second);
    }

    #[test]
    fn activation_verifies_switch_and_rollback_keeps_both_trees_recoverable() {
        let root = temp_dir("activation");
        fs::create_dir_all(root.join("canonical/records")).unwrap();
        fs::create_dir_all(root.join("candidate/records")).unwrap();
        let source = root.join("canonical/records/C-20261101T120000-aaaaaaaa.md");
        fs::write(&source, record(Lib::Canonical, "C-20261101T120000-aaaaaaaa", "X_UNKNOWN=保留", "旧正文")).unwrap();
        let manifest = migration::scan(&root).unwrap();
        let staging = root.parent().unwrap().join(format!("{}-activation-stage", root.file_name().unwrap().to_string_lossy()));
        let staged = build_staged(&manifest, &staging).unwrap();
        let receipt = activate_staged(&staged).unwrap();
        assert_eq!(receipt.phase, "VERIFIED");
        assert!(root.join("planes/long_term/records").is_dir());
        assert!(receipt.backup_root.is_dir());
        verify_activated_root(&root, &staged).unwrap();
        let abandoned = rollback_activation(&root, &receipt.backup_root).unwrap();
        assert!(abandoned.is_dir());
        assert!(root.join("canonical/records/C-20261101T120000-aaaaaaaa.md").is_file());
        assert_eq!(fs::read_to_string(root.join("canonical/records/C-20261101T120000-aaaaaaaa.md")).unwrap(), fs::read_to_string(&source).unwrap());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(abandoned);
        let _ = fs::remove_dir_all(staging);
    }

    #[test]
    fn recovery_receipt_rolls_back_when_live_was_moved_but_stage_not_installed() {
        let root = temp_dir("recovery");
        fs::create_dir_all(root.join("canonical/records")).unwrap();
        fs::create_dir_all(root.join("candidate/records")).unwrap();
        let source = root.join("canonical/records/C-20261101T120000-aaaaaaaa.md");
        fs::write(&source, record(Lib::Canonical, "C-20261101T120000-aaaaaaaa", "", "旧正文")).unwrap();
        let parent = root.parent().unwrap().to_path_buf();
        let backup = parent.join(".abook-test-recovery-backup");
        fs::rename(&root, &backup).unwrap();
        let marker = activation_marker(&parent, "test-recovery");
        let receipt = ActivationReceipt {
            id: "test-recovery".into(),
            live_root: root.clone(),
            staging_root: parent.join("missing-stage"),
            backup_root: backup.clone(),
            old_tree_sha256: tree_digest(&backup).unwrap(),
            new_tree_sha256: "new-tree-not-present".into(),
            phase: "SOURCE_MOVED".into(),
        };
        write_receipt(&marker, &receipt).unwrap();
        let recovered = recover_activation(&root).unwrap();
        assert_eq!(recovered.phase, "RECOVERED_ROLLED_BACK");
        assert!(root.join("canonical/records/C-20261101T120000-aaaaaaaa.md").is_file());
        assert!(!marker.exists());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(backup);
    }

    #[test]
    fn recovery_cleans_prepared_receipt_when_live_remains_old_and_backup_missing() {
        let root = temp_dir("recovery-prepared");
        fs::create_dir_all(root.join("planes/long_term/records")).unwrap();
        fs::write(root.join("planes/long_term/records/old.md"), "old").unwrap();
        let parent = root.parent().unwrap().to_path_buf();
        let marker = activation_marker(&parent, "test-recovery-prepared");
        let receipt = ActivationReceipt {
            id: "test-recovery-prepared".into(),
            live_root: root.clone(),
            staging_root: parent.join("missing-stage"),
            backup_root: parent.join("missing-backup"),
            old_tree_sha256: tree_digest(&root).unwrap(),
            new_tree_sha256: "new-tree-not-installed".into(),
            phase: "PREPARED".into(),
        };
        write_receipt(&marker, &receipt).unwrap();
        let recovered = recover_activation(&root).unwrap();
        assert_eq!(recovered.phase, "RECOVERED_NOT_STARTED");
        assert!(!marker.exists());
        assert!(root.join("planes/long_term/records/old.md").is_file());
        let _ = fs::remove_dir_all(root);
    }


        let root = temp_dir("recovery-committed");
        let parent = root.parent().unwrap().to_path_buf();
        let backup = parent.join(".abook-test-recovery-committed-backup");
        fs::create_dir_all(root.join("planes/long_term/records")).unwrap();
        fs::write(root.join("planes/long_term/records/new.md"), "new").unwrap();
        fs::create_dir_all(backup.join("planes/long_term/records")).unwrap();
        fs::write(backup.join("planes/long_term/records/old.md"), "old").unwrap();
        let marker = activation_marker(&parent, "test-recovery-committed");
        let receipt = ActivationReceipt {
            id: "test-recovery-committed".into(),
            live_root: root.clone(),
            staging_root: parent.join("missing-stage"),
            backup_root: backup.clone(),
            old_tree_sha256: tree_digest(&backup).unwrap(),
            new_tree_sha256: tree_digest(&root).unwrap(),
            phase: "LIVE_MOVED".into(),
        };
        write_receipt(&marker, &receipt).unwrap();
        let recovered = recover_activation(&root).unwrap();
        assert_eq!(recovered.phase, "RECOVERED_COMMITTED");
        assert!(!marker.exists());
        assert!(root.join("planes/long_term/records/new.md").is_file());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(backup);
    }

    #[test]
    fn recovery_rejects_digest_mismatch_and_keeps_receipt() {
        let root = temp_dir("recovery-mismatch");
        let parent = root.parent().unwrap().to_path_buf();
        let backup = parent.join(".abook-test-recovery-mismatch-backup");
        fs::create_dir_all(root.join("planes/long_term/records")).unwrap();
        fs::write(root.join("planes/long_term/records/new.md"), "tampered").unwrap();
        fs::create_dir_all(backup.join("planes/long_term/records")).unwrap();
        fs::write(backup.join("planes/long_term/records/old.md"), "old").unwrap();
        let marker = activation_marker(&parent, "test-recovery-mismatch");
        let receipt = ActivationReceipt {
            id: "test-recovery-mismatch".into(),
            live_root: root.clone(),
            staging_root: parent.join("missing-stage"),
            backup_root: backup.clone(),
            old_tree_sha256: tree_digest(&backup).unwrap(),
            new_tree_sha256: "wrong-digest".into(),
            phase: "LIVE_MOVED".into(),
        };
        write_receipt(&marker, &receipt).unwrap();
        assert!(recover_activation(&root).is_err());
        assert!(marker.is_file());
        let _ = fs::remove_file(marker);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(backup);
    }
    #[test]
    fn failed_build_does_not_create_requested_root_or_touch_source() {
        let root = temp_dir("boundary");
        fs::create_dir_all(root.join("canonical/records")).unwrap();
        fs::create_dir_all(root.join("candidate/records")).unwrap();
        let source = root.join("canonical/records/C-20261101T120000-aaaaaaaa.md");
        fs::write(&source, record(Lib::Canonical, "C-20261101T120000-aaaaaaaa", "X_UNKNOWN=原文", "正文")).unwrap();
        let mut manifest = migration::scan(&root).unwrap();
        manifest.entries[0].body_sha256 = "0".repeat(64);
        let staging = root.parent().unwrap().join(format!("{}-failed", root.file_name().unwrap().to_string_lossy()));
        assert!(build_staged(&manifest, &staging).is_err());
        assert!(!path_exists_any(&staging));
        assert!(source.is_file());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(staging);
    }

    #[test]
    fn staging_under_source_is_rejected_before_write() {
        let root = temp_dir("inside");
        fs::create_dir_all(root.join("canonical/records")).unwrap();
        fs::create_dir_all(root.join("candidate/records")).unwrap();
        let source = root.join("canonical/records/C-20261101T120000-aaaaaaaa.md");
        fs::write(&source, record(Lib::Canonical, "C-20261101T120000-aaaaaaaa", "", "正文")).unwrap();
        let manifest = migration::scan(&root).unwrap();
        let staging = root.join("nested-staging");
        assert!(build_staged(&manifest, &staging).is_err());
        assert!(!path_exists_any(&staging));
        let _ = fs::remove_dir_all(root);
    }
}
