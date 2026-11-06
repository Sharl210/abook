//! 迁移器的**只读侦察层**：scan → manifest。
//!
//! 这个模块只回答四个问题，不执行任何写入：
//!
//! 1. 每条旧记录的**源状态**分类（`source_class` / `source_basis`），用于判断
//!    它是不是墓碑、已晋升/已合并候选、未晋升候选或正式记录；
//! 2. 每条记录的**目标平面**（`target_plane`）和**目标依据**
//!    （`classification_basis`），两者都由字段和显式词表确定，不做临场猜；
//! 3. 旧 ID、文件名骨干、短号、相对路径分别指向哪些记录（多值，不覆盖）；
//! 4. 哪些记录在 `PROMOTED_FROM` 上重复认领、正文 SHA 不一致或完全相同。
//!
//! ## 接线状态
//!
//! 本文件目前**只接到 crate 编译层**（`main.rs` 的 `mod migration;`），没有新增
//! CLI 子命令，也没有改 `routes()`、旧命令或任何写路径。原因不是做不到，而是
//! 现在库里的写入只有单文件原子写和 `LibLock`，没有批量 journal / 批次
//! revision / 全有全无事务；把半成品迁移器挂到 CLI 上，一旦中途失败就会留下
//! "一部分已迁、一部分没迁"的库，比不接线更危险。等批量事务层就绪后，新子命令
//! 只需要调用 [`scan`] 并把 [`Manifest`] 渲染出来；真正的写入器应另建文件，
//! 不应把写逻辑塞回这里。
//!
//! ## 读接口来源
//!
//! - [`crate::rec::Record`] / [`crate::rec::Lib`]：记录解析、`ID`、`PROMOTED_FROM`、
//!   `PROMOTED_TO`、`MERGED_INTO`、`[PROMOTION]` 尾块、文件名与正文；
//! - [`crate::parse`]：容错解析（旧库的 `ID=x;PROJECT=y` 打包行、别名键）；
//! - [`crate::vault`]：`scan_vault` 的只读盘点，`identity_row` / `IDENTITY_SECTION`
//!   用来标出 Global.md 身份索引需要重建，但**不解密正文、不写回**；
//! - [`crate::atlas`]：`Atlas::load` / `Branch::paths` / `branch_of` 的只读地图对照；
//! - [`crate::num::Numbers`]：只读 `load` + `num_of`；`.numbers` 不存在时不会
//!   为了扫描去建表（建表本身是写操作）。
//!
//! 所有函数都不调用 `write_atomic`、`Numbers::save`、`Atlas::set`、
//! `vault::write_entry` 或任何 `std::fs::write/create/remove`。

#![allow(dead_code)]

#[path = "migration_write.rs"]
pub mod migration_write;

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};

use crate::atlas::Atlas;
use crate::rec::{Lib, Record};
use crate::sha256;
use crate::util::{list_md, notfound, rel, R};

// ---------------------------------------------------------------------------
// 记录身份
// ---------------------------------------------------------------------------

/// 一条旧记录在 manifest 内部的稳定定位符。
///
/// 不用 `Record::id()`：旧库的 `ID` 字段可能是 `AB2-C-001`，而文件名可能是
/// 另一个别名；迁移只读阶段需要的是"磁盘上这一条"，不是某个可能被写坏的字段。
/// `lib + 文件名骨干` 唯一且不会因为正文变化而漂移。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordKey {
    pub lib: Lib,
    pub stem: String,
}

impl RecordKey {
    pub fn relative_path(&self) -> String {
        format!("{}/{}.md", self.lib.dir(), self.stem)
    }
}

impl fmt::Display for RecordKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.lib.dir(), self.stem)
    }
}

fn lib_rank(lib: Lib) -> u8 {
    match lib {
        Lib::Canonical => 0,
        Lib::Candidate => 1,
    }
}

impl PartialOrd for RecordKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RecordKey {
    fn cmp(&self, other: &Self) -> Ordering {
        lib_rank(self.lib)
            .cmp(&lib_rank(other.lib))
            .then_with(|| self.stem.cmp(&other.stem))
    }
}

fn record_key(rec: &Record) -> RecordKey {
    let mut stem = rec.file_name();
    if let Some(s) = stem.strip_suffix(".md") {
        stem = s.to_string();
    }
    if stem.is_empty() {
        stem = rec.id();
    }
    RecordKey {
        lib: rec.lib,
        stem,
    }
}

// ---------------------------------------------------------------------------
// 多值旧 ID 映射
// ---------------------------------------------------------------------------

/// 一个别名是从哪个口径发现的。只用于解释，不参与冲突判定时的取舍。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AliasSource {
    /// 记录头部的 `ID` 字段（可能带 `ID=` 前缀或打包行）。
    HeaderId,
    /// 文件名骨干（`foo.md` → `foo`）。
    FileStem,
    /// `.numbers` 里已有的短号（`C123` / `N45`）。
    ShortNumber,
    /// 相对库根的路径（`canonical/records/foo.md`）。
    RelativePath,
}

impl AliasSource {
    pub fn as_str(self) -> &'static str {
        match self {
            AliasSource::HeaderId => "header_id",
            AliasSource::FileStem => "file_stem",
            AliasSource::ShortNumber => "short_number",
            AliasSource::RelativePath => "relative_path",
        }
    }
}

/// 一个旧别名指向的一条记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdTarget {
    pub key: RecordKey,
    /// 同一个别名可能同时是这条记录的头 ID 和文件名骨干；合并保留，不丢来源。
    pub sources: BTreeSet<AliasSource>,
}

/// 旧别名 → 可能的多条记录。
///
/// **只增不覆盖**是这张表的全部意义。旧库里同一个 `AB2-C-001` 可能同时出现
/// 在两条记录上（重复文件名、复制出来的副本、历史手写冲突），迁移时不能因为
/// "后读到的覆盖先读到的"就悄悄丢掉一条。所有查询都返回切片；调用方必须自己
/// 决定是一对一还是需要报冲突。
#[derive(Clone, Debug, Default)]
pub struct OldIdMap {
    by_alias: BTreeMap<String, Vec<IdTarget>>,
}

impl OldIdMap {
    pub fn targets(&self, alias: &str) -> &[IdTarget] {
        self.by_alias
            .get(alias.trim())
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn aliases(&self) -> impl Iterator<Item = &String> {
        self.by_alias.keys()
    }

    pub fn len(&self) -> usize {
        self.by_alias.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_alias.is_empty()
    }

    pub fn target_count(&self) -> usize {
        self.by_alias.values().map(Vec::len).sum()
    }

    /// 同一个别名指向多于一条记录时，原样交出来。这里不替调用方选。
    pub fn ambiguous(&self) -> Vec<AmbiguousAlias> {
        let mut out = Vec::new();
        for (alias, targets) in &self.by_alias {
            if targets.len() > 1 {
                out.push(AmbiguousAlias {
                    alias: alias.clone(),
                    targets: targets.clone(),
                });
            }
        }
        out
    }

    fn add(&mut self, alias: &str, key: &RecordKey, source: AliasSource) {
        let alias = alias.trim();
        if alias.is_empty() {
            return;
        }
        let targets = self.by_alias.entry(alias.to_string()).or_default();
        if let Some(existing) = targets.iter_mut().find(|t| &t.key == key) {
            existing.sources.insert(source);
            return;
        }
        let mut sources = BTreeSet::new();
        sources.insert(source);
        targets.push(IdTarget {
            key: key.clone(),
            sources,
        });
        targets.sort_by(|a, b| a.key.cmp(&b.key));
    }
}

#[derive(Clone, Debug)]
pub struct AmbiguousAlias {
    pub alias: String,
    pub targets: Vec<IdTarget>,
}

// ---------------------------------------------------------------------------
// 分类
// ---------------------------------------------------------------------------

/// 迁移分类。声明顺序就是固定优先级，**先命中者赢**。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceRecordClass {
    /// `STATUS=DELETED` 的墓碑。任何其它标记都不能把它复活。
    Tombstone,
    /// 正式库记录。
    Canonical,
    /// 候选库中已通过 `MERGED_INTO` 并进另一条候选的记录。
    CandidateMerged,
    /// 候选库中已有晋升证据的记录（尾块 / `PROMOTED_TO` / 正式库回指）。
    CandidatePromoted,
    /// 候选库中没有任何晋升证据的记录；迁移时应保留为 episodic。
    CandidateUnpromoted,
}

impl SourceRecordClass {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceRecordClass::Tombstone => "TOMBSTONE",
            SourceRecordClass::Canonical => "CANONICAL",
            SourceRecordClass::CandidateMerged => "CANDIDATE_MERGED",
            SourceRecordClass::CandidatePromoted => "CANDIDATE_PROMOTED",
            SourceRecordClass::CandidateUnpromoted => "CANDIDATE_UNPROMOTED",
        }
    }

    pub fn priority(self) -> usize {
        match self {
            SourceRecordClass::Tombstone => 0,
            SourceRecordClass::Canonical => 1,
            SourceRecordClass::CandidateMerged => 2,
            SourceRecordClass::CandidatePromoted => 3,
            SourceRecordClass::CandidateUnpromoted => 4,
        }
    }
}

/// 固定分类优先级。测试会锁死这个顺序；改顺序必须同时改迁移规则。
pub const SOURCE_CLASSIFICATION_PRIORITY: [SourceRecordClass; 5] = [
    SourceRecordClass::Tombstone,
    SourceRecordClass::Canonical,
    SourceRecordClass::CandidateMerged,
    SourceRecordClass::CandidatePromoted,
    SourceRecordClass::CandidateUnpromoted,
];

/// 分类依据。枚举声明顺序也是依据优先级；`PromotedFrom` 里带的是正式库记录，
/// 因此同一条候选被多条正式记录认领时，依据本身就能说明"多值"而不是被折叠。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceBasis {
    StatusDeleted,
    CanonicalLibrary,
    MergedInto(String),
    PromotionBlock,
    PromotedTo(String),
    PromotedFrom(Vec<RecordKey>),
    NoPromotionMarker,
}

impl fmt::Display for SourceBasis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceBasis::StatusDeleted => f.write_str("STATUS=DELETED"),
            SourceBasis::CanonicalLibrary => f.write_str("LIB=CANONICAL"),
            SourceBasis::MergedInto(v) => write!(f, "MERGED_INTO={v}"),
            SourceBasis::PromotionBlock => f.write_str("PROMOTION_BLOCK"),
            SourceBasis::PromotedTo(v) => write!(f, "PROMOTED_TO={v}"),
            SourceBasis::PromotedFrom(keys) => {
                let joined = keys
                    .iter()
                    .map(|k| k.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                write!(f, "PROMOTED_FROM_BACKREF={joined}")
            }
            SourceBasis::NoPromotionMarker => f.write_str("NO_PROMOTION_MARKER"),
        }
    }
}

// ---------------------------------------------------------------------------
// 目标平面
// ---------------------------------------------------------------------------

/// 迁移目标平面。固定顺序就是判定优先级，先命中者赢。
///
/// `Tombstone` 是迁移落点，不是六个记忆平面之一；`MEMORY_PLANES` 才是六平面。
/// 把它放进同一个枚举，是为了让每条 manifest 条目都有一个确定的目标值，
/// 不必用 `Option` 让调用方自己解释“没有平面”是墓碑还是漏判。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TargetPlane {
    Tombstone,
    Rules,
    Profile,
    Explicit,
    Soul,
    Episodic,
    LongTerm,
}

impl TargetPlane {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetPlane::Tombstone => "tombstone",
            TargetPlane::Rules => "rules",
            TargetPlane::Profile => "profile",
            TargetPlane::Explicit => "explicit",
            TargetPlane::Soul => "soul",
            TargetPlane::Episodic => "episodic",
            TargetPlane::LongTerm => "long_term",
        }
    }

    pub fn is_memory_plane(self) -> bool {
        !matches!(self, TargetPlane::Tombstone)
    }
}

/// 六平面。不包含墓碑。
pub const MEMORY_PLANES: [TargetPlane; 6] = [
    TargetPlane::Rules,
    TargetPlane::Profile,
    TargetPlane::Explicit,
    TargetPlane::Soul,
    TargetPlane::Episodic,
    TargetPlane::LongTerm,
];

/// 目标判定优先级。墓碑排第一；其余与 [`MEMORY_PLANES`] 同序。
pub const TARGET_PLANE_PRIORITY: [TargetPlane; 7] = [
    TargetPlane::Tombstone,
    TargetPlane::Rules,
    TargetPlane::Profile,
    TargetPlane::Explicit,
    TargetPlane::Soul,
    TargetPlane::Episodic,
    TargetPlane::LongTerm,
];

/// 目标平面的判定依据。它回答的是“为什么去这个平面”，与源状态分类无关。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TargetBasis {
    StatusDeleted,
    KindConstraintUserAuthority,
    ExplicitHardRule,
    UserProfileSignal,
    AuthorityUserExplicit,
    AiIdentitySignal,
    AiStyleSignal,
    AiWorkPrincipleSignal,
    UnpromotedCandidate,
    CanonicalEpisodicKindWithSession,
    PromotedCandidateProvenance,
    CanonicalFallbackLongTerm,
}

impl TargetBasis {
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetBasis::StatusDeleted => "STATUS_DELETED",
            TargetBasis::KindConstraintUserAuthority => "KIND_CONSTRAINT_USER_AUTHORITY",
            TargetBasis::ExplicitHardRule => "EXPLICIT_HARD_RULE",
            TargetBasis::UserProfileSignal => "USER_PROFILE_SIGNAL",
            TargetBasis::AuthorityUserExplicit => "AUTHORITY_USER_EXPLICIT",
            TargetBasis::AiIdentitySignal => "AI_IDENTITY_SIGNAL",
            TargetBasis::AiStyleSignal => "AI_STYLE_SIGNAL",
            TargetBasis::AiWorkPrincipleSignal => "AI_WORK_PRINCIPLE_SIGNAL",
            TargetBasis::UnpromotedCandidate => "UNPROMOTED_CANDIDATE",
            TargetBasis::CanonicalEpisodicKindWithSession => {
                "CANONICAL_EPISODIC_KIND_WITH_SESSION"
            }
            TargetBasis::PromotedCandidateProvenance => "PROMOTED_CANDIDATE_PROVENANCE",
            TargetBasis::CanonicalFallbackLongTerm => "CANONICAL_FALLBACK_LONG_TERM",
        }
    }
}

impl fmt::Display for TargetBasis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// manifest 条目
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ManifestEntry {
    pub key: RecordKey,
    pub path: PathBuf,
    pub path_rel: String,

    /// 头部 `ID` 字段拆出来的全部别名（旧库可能是 `ID=x;PROJECT=y` 打包行）。
    pub legacy_header_ids: Vec<String>,
    pub short_number: Option<String>,

    pub title: String,
    pub kind: String,
    pub maturity: String,
    /// 头部原始 `STATUS`，不做 `Record::status()` 的合并；墓碑判定另有依据。
    pub status: String,

    pub source_class: SourceRecordClass,
    pub source_basis: SourceBasis,
    /// 所有观察到的依据，按固定优先级排列；`source_basis` 是第一项。
    pub source_evidence: Vec<SourceBasis>,

    /// 目标平面。`Tombstone` 是迁移落点，不属于六个记忆平面。
    pub target_plane: TargetPlane,
    /// 目标平面的判定依据；不是旧 `source_class` 的依据。
    pub classification_basis: TargetBasis,

    /// 正文 SHA-256（换行统一、首尾空白去掉后计算；内部空白保留）。
    pub body_sha256: String,
    pub body_bytes: usize,
    pub body_lines: usize,

    pub promoted_from: Vec<String>,
    pub promoted_from_targets: Vec<RecordKey>,
    pub promoted_by: Vec<RecordKey>,
    pub promoted_to: Vec<String>,
    pub merged_into: Vec<String>,
    pub has_promotion_block: bool,

    pub atlas_branch: Option<String>,
    pub header_inferred: bool,
    pub deviation_count: usize,
}

impl ManifestEntry {
    pub fn source_class_label(&self) -> &'static str {
        self.source_class.as_str()
    }

    pub fn source_basis_label(&self) -> String {
        self.source_basis.to_string()
    }

    pub fn target_plane_label(&self) -> &'static str {
        self.target_plane.as_str()
    }

    pub fn target_basis_label(&self) -> String {
        self.classification_basis.to_string()
    }
}

#[derive(Clone, Debug)]
pub struct PromotionEdge {
    pub canonical: RecordKey,
    pub raw: String,
    pub values: Vec<String>,
    pub id_values: Vec<String>,
    /// 旧库 `PROMOTED_FROM` 里常见的来源文档路径/自然语言描述。它们不是记录 ID，
    /// 不参与指认，但原样留在 manifest 里，避免"看起来像没读到"。
    pub non_id_values: Vec<String>,
    pub resolved_candidates: Vec<RecordKey>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelationTarget {
    Resolved(RecordKey),
    Ambiguous(Vec<RecordKey>),
    Unresolved,
    NonIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationEdge {
    pub source: RecordKey,
    pub field: &'static str,
    pub ordinal: usize,
    pub value: String,
    pub target: RelationTarget,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromotionIssueKind {
    /// 值长得像身份，但全库没有任何记录认领它。
    Unresolved,
    /// 同一值命中多条候选记录；不猜，交给人。
    Ambiguous,
    /// 值只命中正式记录，没有候选目标。
    NoCandidateTarget,
}

impl PromotionIssueKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PromotionIssueKind::Unresolved => "unresolved",
            PromotionIssueKind::Ambiguous => "ambiguous",
            PromotionIssueKind::NoCandidateTarget => "no_candidate_target",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PromotionIssue {
    pub canonical: RecordKey,
    pub value: String,
    pub kind: PromotionIssueKind,
    pub targets: Vec<RecordKey>,
}

// ---------------------------------------------------------------------------
// 冲突报告
// ---------------------------------------------------------------------------

/// 多条正式记录重复认领同一个 `PROMOTED_FROM` 值。
#[derive(Clone, Debug)]
pub struct DuplicatePromotedFrom {
    pub value: String,
    pub claimed_by: Vec<RecordKey>,
    pub body_shas: BTreeMap<RecordKey, String>,
    /// 组内只出现在一条正文里的非空行数；0 表示正文完全一致。
    pub unique_lines: usize,
}

impl DuplicatePromotedFrom {
    pub fn has_body_conflict(&self) -> bool {
        self.body_shas.values().collect::<BTreeSet<_>>().len() > 1
    }
}

/// 一条正式记录和它 `PROMOTED_FROM` 指向的候选正文 SHA 不同。
///
/// 这不是哈希碰撞；它表示"同一次晋升的两侧，正文已经分叉"。迁移必须保留两条
/// 并建立关系，不能按时间或长度静默选一版。
#[derive(Clone, Debug)]
pub struct BodyShaDivergence {
    pub canonical: RecordKey,
    pub candidate: RecordKey,
    pub canonical_sha256: String,
    pub candidate_sha256: String,
    pub canonical_only_lines: usize,
    pub candidate_only_lines: usize,
}

/// 不同记录正文 SHA 完全相同。可能是合法副本，也可能是重复写入；只报告，不删。
#[derive(Clone, Debug)]
pub struct DuplicateBodySha {
    pub body_sha256: String,
    pub records: Vec<RecordKey>,
    pub body_lines: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ConflictReport {
    pub duplicate_promoted_from: Vec<DuplicatePromotedFrom>,
    pub ambiguous_aliases: Vec<AmbiguousAlias>,
    pub promotion_issues: Vec<PromotionIssue>,
    pub body_sha_divergences: Vec<BodyShaDivergence>,
    pub duplicate_body_sha: Vec<DuplicateBodySha>,
}

impl ConflictReport {
    pub fn total(&self) -> usize {
        self.duplicate_promoted_from.len()
            + self.ambiguous_aliases.len()
            + self.promotion_issues.len()
            + self.body_sha_divergences.len()
            + self.duplicate_body_sha.len()
    }

    pub fn body_divergence_unique_lines(&self) -> usize {
        self.body_sha_divergences
            .iter()
            .map(|d| d.canonical_only_lines + d.candidate_only_lines)
            .sum()
    }

    pub fn duplicate_promoted_from_body_conflict_count(&self) -> usize {
        self.duplicate_promoted_from
            .iter()
            .filter(|g| g.has_body_conflict())
            .count()
    }

    pub fn duplicate_promoted_from_unique_lines(&self) -> usize {
        self.duplicate_promoted_from
            .iter()
            .filter(|g| g.has_body_conflict())
            .map(|g| g.unique_lines)
            .sum()
    }
}

// ---------------------------------------------------------------------------
// Atlas / Vault / 未读文件
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct UnreadableRecord {
    pub path: PathBuf,
    pub lib: Lib,
    pub error: String,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RecordDirPresence {
    pub canonical: bool,
    pub candidate: bool,
}

impl RecordDirPresence {
    pub fn missing(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if !self.canonical {
            out.push(Lib::Canonical.dir());
        }
        if !self.candidate {
            out.push(Lib::Candidate.dir());
        }
        out
    }
}

#[derive(Clone, Debug)]
pub struct AtlasPath {
    pub branch: String,
    pub path: String,
}

#[derive(Clone, Debug, Default)]
pub struct AtlasScan {
    pub loaded: bool,
    pub branches: usize,
    pub paths: Vec<AtlasPath>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct AtlasLibSummary {
    pub loaded: bool,
    pub branches: usize,
    pub mapped_paths: usize,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AtlasDanglingPath {
    pub lib: Lib,
    pub branch: String,
    pub path: String,
}

#[derive(Clone, Debug, Default)]
pub struct AtlasManifest {
    pub canonical: AtlasLibSummary,
    pub candidate: AtlasLibSummary,
    pub unmapped_records: Vec<RecordKey>,
    pub dangling_paths: Vec<AtlasDanglingPath>,
}

#[derive(Clone, Debug)]
pub struct VaultEntrySummary {
    pub ident: String,
    pub purpose: String,
    pub verified: bool,
}

#[derive(Clone, Debug)]
pub struct BrokenVaultEntry {
    pub file: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct VaultManifest {
    pub dir_present: bool,
    pub entries: Vec<VaultEntrySummary>,
    pub broken: Vec<BrokenVaultEntry>,
    pub identity_rows: Vec<String>,
    /// `Global.md` 里是否已有 `## Credential Identity Index` 节。
    pub global_has_identity_section: bool,
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Manifest {
    pub root: PathBuf,
    pub entries: Vec<ManifestEntry>,
    pub unreadable: Vec<UnreadableRecord>,
    pub record_dirs: RecordDirPresence,
    pub old_id_map: OldIdMap,
    pub promotion_edges: Vec<PromotionEdge>,
    pub relation_edges: Vec<RelationEdge>,
    pub conflicts: ConflictReport,
    pub atlas: AtlasManifest,
    pub vault: VaultManifest,
}

impl Manifest {
    pub fn source_class_count(&self, class: SourceRecordClass) -> usize {
        self.entries.iter().filter(|e| e.source_class == class).count()
    }

    pub fn target_plane_count(&self, plane: TargetPlane) -> usize {
        self.entries.iter().filter(|e| e.target_plane == plane).count()
    }

    pub fn entry(&self, key: &RecordKey) -> Option<&ManifestEntry> {
        self.entries.iter().find(|e| &e.key == key)
    }

    /// 逐条清单：路径、源分类、目标平面、目标依据、正文 SHA、被几条正式记录回指。
    ///
    /// 这是给未来 `migrate scan` 的 `--tsv` / 人工核对用的；不写文件。
    pub fn render_entries(&self) -> String {
        let mut out = String::new();
        for e in &self.entries {
            let _ = writeln!(
                out,
                "{}\t{}\t{}\t{}\t{}\t{}",
                e.path_rel,
                e.source_class.as_str(),
                e.target_plane.as_str(),
                e.target_basis_label(),
                e.body_sha256,
                e.promoted_by.len()
            );
        }
        out
    }

    /// 只是给人看的只读摘要；真正的交付物是结构和 `ManifestEntry` 列表。
    pub fn render_report(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "MIGRATION MANIFEST (READ-ONLY)");
        let _ = writeln!(out, "root: {}", self.root.display());
        let canonical = self
            .entries
            .iter()
            .filter(|e| e.key.lib == Lib::Canonical)
            .count();
        let candidate = self.entries.len().saturating_sub(canonical);
        let _ = writeln!(
            out,
            "records: {} (canonical={}, candidate={}); unreadable={}",
            self.entries.len(),
            canonical,
            candidate,
            self.unreadable.len()
        );
        let missing = self.record_dirs.missing();
        if !missing.is_empty() {
            let _ = writeln!(out, "missing record dirs: {}", missing.join(", "));
        }
        let _ = writeln!(out, "source classification:");
        for class in SOURCE_CLASSIFICATION_PRIORITY {
            let _ = writeln!(out, "  {:<22} {}", class.as_str(), self.source_class_count(class));
        }
        let _ = writeln!(out, "target planes:");
        for plane in TARGET_PLANE_PRIORITY {
            let _ = writeln!(out, "  {:<22} {}", plane.as_str(), self.target_plane_count(plane));
        }
        let _ = writeln!(
            out,
            "old-id aliases: {} aliases / {} targets; ambiguous={}",
            self.old_id_map.len(),
            self.old_id_map.target_count(),
            self.conflicts.ambiguous_aliases.len()
        );
        let _ = writeln!(
            out,
            "promotion edges: {}; duplicate PROMOTED_FROM groups: {} (body-conflict {}, unique lines {})",
            self.promotion_edges.len(),
            self.conflicts.duplicate_promoted_from.len(),
            self.conflicts.duplicate_promoted_from_body_conflict_count(),
            self.conflicts.duplicate_promoted_from_unique_lines()
        );
        let _ = writeln!(
            out,
            "conflicts: body-sha divergences={} (unique lines {}); duplicate body-sha groups={}; promotion issues={}",
            self.conflicts.body_sha_divergences.len(),
            self.conflicts.body_divergence_unique_lines(),
            self.conflicts.duplicate_body_sha.len(),
            self.conflicts.promotion_issues.len()
        );
        let _ = writeln!(
            out,
            "atlas: canonical loaded={} branches={} mapped-paths={}; candidate loaded={} branches={} mapped-paths={}; unmapped records={}; dangling paths={}",
            self.atlas.canonical.loaded,
            self.atlas.canonical.branches,
            self.atlas.canonical.mapped_paths,
            self.atlas.candidate.loaded,
            self.atlas.candidate.branches,
            self.atlas.candidate.mapped_paths,
            self.atlas.unmapped_records.len(),
            self.atlas.dangling_paths.len()
        );
        let _ = writeln!(
            out,
            "vault: dir-present={} entries={} broken={} identity-rows={} global-identity-section={}",
            self.vault.dir_present,
            self.vault.entries.len(),
            self.vault.broken.len(),
            self.vault.identity_rows.len(),
            self.vault.global_has_identity_section
        );
        out
    }
}

// ---------------------------------------------------------------------------
// 扫描入口
// ---------------------------------------------------------------------------

/// 扫描入口。**只读**：不会创建 `.numbers`、不会刷新 Atlas、不会写 Global，
/// 也不会碰任何记录文件。
pub fn scan(root: &Path) -> R<Manifest> {
    if !root.is_dir() {
        return notfound(format!("迁移扫描根目录不存在: {}", root.display()));
    }

    let mut records = Vec::new();
    let mut unreadable = Vec::new();
    for lib in [Lib::Canonical, Lib::Candidate] {
        for path in list_md(&root.join(lib.dir())) {
            match Record::parse(&path, lib) {
                Ok(rec) => records.push(rec),
                Err(e) => unreadable.push(UnreadableRecord {
                    path,
                    lib,
                    error: e.msg().to_string(),
                }),
            }
        }
    }

    let short_numbers = collect_short_numbers(root, &records);
    let record_dirs = RecordDirPresence {
        canonical: root.join(Lib::Canonical.dir()).is_dir(),
        candidate: root.join(Lib::Candidate.dir()).is_dir(),
    };

    Ok(build_manifest(ScanInput {
        root: root.to_path_buf(),
        records,
        unreadable,
        short_numbers,
        atlas_canonical: scan_atlas(root, Lib::Canonical),
        atlas_candidate: scan_atlas(root, Lib::Candidate),
        vault: scan_vault_manifest(root),
        record_dirs,
    }))
}

/// 从已解析的记录直接建 manifest。单元测试用它；将来的 `--dry-run` 也可以
/// 复用，不必为了测试先造一个临时库。
pub fn manifest_from_records(records: Vec<Record>) -> Manifest {
    build_manifest(ScanInput {
        records,
        ..ScanInput::default()
    })
}

#[derive(Clone, Debug, Default)]
struct ScanInput {
    root: PathBuf,
    records: Vec<Record>,
    unreadable: Vec<UnreadableRecord>,
    short_numbers: BTreeMap<RecordKey, String>,
    atlas_canonical: AtlasScan,
    atlas_candidate: AtlasScan,
    vault: VaultManifest,
    record_dirs: RecordDirPresence,
}

fn build_manifest(input: ScanInput) -> Manifest {
    let root = input.root.clone();
    let records = input.records;

    // 先建 key → 记录，后面的冲突计数、正文对比都靠它。
    let mut records_by_key: BTreeMap<RecordKey, &Record> = BTreeMap::new();
    for rec in &records {
        records_by_key.insert(record_key(rec), rec);
    }

    let relation_input: Vec<(RecordKey, &Record)> = records.iter().map(|rec| (record_key(rec), rec)).collect();
    let relation_edges = relation_edges_from_records(&relation_input);

    // ---- 1. 多值旧 ID 映射：只增别名，不覆盖逻辑 ID
    let mut old_id_map = OldIdMap::default();
    for rec in &records {
        let key = record_key(rec);
        for id in header_id_aliases(rec) {
            old_id_map.add(&id, &key, AliasSource::HeaderId);
        }
        old_id_map.add(&key.stem, &key, AliasSource::FileStem);
        if let Some(num) = input.short_numbers.get(&key) {
            old_id_map.add(num, &key, AliasSource::ShortNumber);
        }
        let path_rel = rel(&root, &rec.path);
        if !path_rel.is_empty() {
            old_id_map.add(&path_rel, &key, AliasSource::RelativePath);
        }
    }

    // ---- 2. PROMOTED_FROM：多值、逐条解析、逐条解析目标
    let mut promotion_edges = Vec::new();
    let mut backrefs: BTreeMap<RecordKey, Vec<RecordKey>> = BTreeMap::new();
    let mut promotion_issues = Vec::new();
    for rec in &records {
        if rec.lib != Lib::Canonical {
            continue;
        }
        let raw = rec.get("PROMOTED_FROM").trim().to_string();
        if raw.is_empty() {
            continue;
        }
        let key = record_key(rec);
        let values = parse_ref_values(&raw);
        let mut id_values = Vec::new();
        let mut non_id_values = Vec::new();
        let mut resolved_candidates = Vec::new();

        for value in &values {
            if !is_ref_shaped(value) {
                non_id_values.push(value.clone());
                continue;
            }
            id_values.push(value.clone());
            let all_targets: Vec<RecordKey> = old_id_map
                .targets(value)
                .iter()
                .map(|t| t.key.clone())
                .collect();
            let candidates = dedup_sorted(
                all_targets
                    .iter()
                    .filter(|k| k.lib == Lib::Candidate)
                    .cloned()
                    .collect(),
            );

            match candidates.len() {
                0 => {
                    let kind = if all_targets.is_empty() {
                        PromotionIssueKind::Unresolved
                    } else {
                        PromotionIssueKind::NoCandidateTarget
                    };
                    promotion_issues.push(PromotionIssue {
                        canonical: key.clone(),
                        value: value.clone(),
                        kind,
                        targets: all_targets,
                    });
                }
                1 => {
                    let cand = candidates[0].clone();
                    if !resolved_candidates.contains(&cand) {
                        resolved_candidates.push(cand.clone());
                    }
                    let by = backrefs.entry(cand).or_default();
                    if !by.contains(&key) {
                        by.push(key.clone());
                    }
                }
                _ => {
                    // 不猜：歧义值不建立回指，候选保持"未晋升"，同时进冲突报告。
                    promotion_issues.push(PromotionIssue {
                        canonical: key.clone(),
                        value: value.clone(),
                        kind: PromotionIssueKind::Ambiguous,
                        targets: candidates,
                    });
                }
            }
        }

        promotion_edges.push(PromotionEdge {
            canonical: key,
            raw,
            values,
            id_values,
            non_id_values,
            resolved_candidates,
        });
    }
    for by in backrefs.values_mut() {
        by.sort();
        by.dedup();
    }
    promotion_issues.sort_by(|a, b| {
        a.canonical
            .cmp(&b.canonical)
            .then_with(|| a.value.cmp(&b.value))
    });

    // ---- 3. 分类 + manifest 条目
    let mut entries = Vec::new();
    for rec in &records {
        let key = record_key(rec);
        let (source_class, source_basis, source_evidence) = classify_source(rec, &key, &backrefs);
        let (target_plane, target_basis) = classify_target(rec, source_class);
        let promoted_from = parse_ref_values(&rec.get("PROMOTED_FROM"));
        let promoted_from_targets = promotion_edges
            .iter()
            .find(|e| e.canonical == key)
            .map(|e| e.resolved_candidates.clone())
            .unwrap_or_default();
        let promoted_by = backrefs.get(&key).cloned().unwrap_or_default();
        let path_rel = {
            let r = rel(&root, &rec.path);
            if r.is_empty() {
                key.relative_path()
            } else {
                r
            }
        };
        entries.push(ManifestEntry {
            key: key.clone(),
            path: rec.path.clone(),
            path_rel,
            legacy_header_ids: header_id_aliases(rec),
            short_number: input.short_numbers.get(&key).cloned(),
            title: rec.title(),
            kind: rec.kind(),
            maturity: rec.get("MATURITY"),
            status: rec.get("STATUS"),
            source_class,
            source_basis,
            source_evidence,
            target_plane,
            classification_basis: target_basis,
            body_sha256: body_digest(&rec.body),
            body_bytes: rec.body.len(),
            body_lines: body_line_count(&rec.body),
            promoted_from,
            promoted_from_targets,
            promoted_by,
            promoted_to: parse_ref_values(&rec.get("PROMOTED_TO")),
            merged_into: parse_ref_values(&rec.get("MERGED_INTO")),
            has_promotion_block: rec.promotion.is_some(),
            atlas_branch: atlas_branch_of(
                if key.lib == Lib::Canonical {
                    &input.atlas_canonical
                } else {
                    &input.atlas_candidate
                },
                &key.stem,
            ),
            header_inferred: rec.header_inferred,
            deviation_count: rec.devs.len(),
        });
    }

    // ---- 4. 冲突
    let conflicts = build_conflicts(
        &entries,
        &records_by_key,
        &old_id_map,
        &promotion_edges,
        promotion_issues,
    );

    // ---- 5. Atlas 最终对照
    let atlas = finalize_atlas(&entries, &input.atlas_canonical, &input.atlas_candidate);

    Manifest {
        root,
        entries,
        unreadable: input.unreadable,
        record_dirs: input.record_dirs,
        old_id_map,
        promotion_edges,
        relation_edges,
        conflicts,
        atlas,
        vault: input.vault,
    }
}

// ---------------------------------------------------------------------------
// 扫描辅助
// ---------------------------------------------------------------------------

fn header_id_aliases(rec: &Record) -> Vec<String> {
    let raw = rec.get("ID");
    let mut out = Vec::new();
    // 旧库的 ID 行可能是 `ID=AB2-C-001;PROJECT=...` 的打包行；解析器通常已经
    // 拆成字段，这里再兜一层，避免 `Record::get("ID")` 拿到整行时把 project 也
    // 当成 ID。
    for part in raw.split([';', '；', ',', '，']) {
        let mut token = part.trim();
        if token.is_empty() {
            continue;
        }
        if let Some(rest) = token.strip_prefix("ID=") {
            token = rest.trim();
        }
        if token.is_empty() || token.contains('=') || token.chars().any(char::is_whitespace) {
            continue;
        }
        push_unique(&mut out, token.to_string());
    }
    let id = rec.id();
    if !id.is_empty() {
        push_unique(&mut out, id);
    }
    out
}

fn collect_short_numbers(root: &Path, records: &[Record]) -> BTreeMap<RecordKey, String> {
    // `load` 只读；`load_or_build`/`save` 会建表写盘，绝不能在这里用。
    let Some(nums) = crate::num::Numbers::load(root) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for rec in records {
        let key = record_key(rec);
        let mut probes = vec![rec.id(), key.stem.clone()];
        probes.extend(header_id_aliases(rec));
        for probe in probes {
            if probe.is_empty() {
                continue;
            }
            if let Some(num) = nums.num_of(&probe) {
                out.insert(key, num.to_string());
                break;
            }
        }
    }
    out
}

fn scan_atlas(root: &Path, lib: Lib) -> AtlasScan {
    let path = root.join(lib.atlas());
    if !path.exists() {
        // 缺地图不是"所有记录都未映射"：`loaded=false` 让调用方自己决定。
        return AtlasScan::default();
    }
    match Atlas::load(&path) {
        Ok(atlas) => {
            let mut paths = Vec::new();
            for branch in &atlas.branches {
                let name = branch.name();
                for p in branch.paths() {
                    paths.push(AtlasPath {
                        branch: name.clone(),
                        path: p,
                    });
                }
            }
            AtlasScan {
                loaded: true,
                branches: atlas.branches.len(),
                paths,
                error: None,
            }
        }
        Err(e) => AtlasScan {
            loaded: false,
            branches: 0,
            paths: Vec::new(),
            error: Some(e.msg().to_string()),
        },
    }
}

fn scan_vault_manifest(root: &Path) -> VaultManifest {
    let (entries, bad) = crate::vault::scan_vault(root);
    let mut summaries = Vec::new();
    let mut identity_rows = Vec::new();
    for entry in &entries {
        summaries.push(VaultEntrySummary {
            ident: entry.ident.clone(),
            purpose: entry.purpose.clone(),
            verified: entry.verified,
        });
        identity_rows.push(crate::vault::identity_row(entry));
    }
    let global_has_identity_section = std::fs::read_to_string(root.join("Global.md"))
        .map(|text| text.contains(crate::vault::IDENTITY_SECTION))
        .unwrap_or(false);
    VaultManifest {
        dir_present: crate::vault::vault_dir(root).is_dir(),
        entries: summaries,
        broken: bad
            .into_iter()
            .map(|(file, reason)| BrokenVaultEntry { file, reason })
            .collect(),
        identity_rows,
        global_has_identity_section,
    }
}

fn finalize_atlas(
    entries: &[ManifestEntry],
    canonical: &AtlasScan,
    candidate: &AtlasScan,
) -> AtlasManifest {
    let mut atlas = AtlasManifest {
        canonical: summarize_atlas(canonical),
        candidate: summarize_atlas(candidate),
        ..AtlasManifest::default()
    };
    let mut mapped: BTreeSet<RecordKey> = BTreeSet::new();

    for (lib, scan) in [
        (Lib::Canonical, canonical),
        (Lib::Candidate, candidate),
    ] {
        if !scan.loaded {
            continue;
        }
        for p in &scan.paths {
            let base = atlas_basename(&p.path);
            let hits: Vec<RecordKey> = entries
                .iter()
                .filter(|e| e.key.lib == lib && e.key.stem == base)
                .map(|e| e.key.clone())
                .collect();
            if hits.is_empty() {
                atlas.dangling_paths.push(AtlasDanglingPath {
                    lib,
                    branch: p.branch.clone(),
                    path: p.path.clone(),
                });
            } else {
                mapped.extend(hits);
            }
        }
    }

    atlas.unmapped_records = entries
        .iter()
        .filter(|e| {
            let scan = if e.key.lib == Lib::Canonical {
                canonical
            } else {
                candidate
            };
            scan.loaded && !mapped.contains(&e.key)
        })
        .map(|e| e.key.clone())
        .collect();

    atlas
}

fn summarize_atlas(scan: &AtlasScan) -> AtlasLibSummary {
    AtlasLibSummary {
        loaded: scan.loaded,
        branches: scan.branches,
        mapped_paths: scan.paths.len(),
        error: scan.error.clone(),
    }
}

fn atlas_branch_of(scan: &AtlasScan, stem: &str) -> Option<String> {
    if !scan.loaded {
        return None;
    }
    scan.paths
        .iter()
        .find(|p| atlas_basename(&p.path) == stem)
        .map(|p| p.branch.clone())
}

fn atlas_basename(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".md")
        .to_string()
}

// ---------------------------------------------------------------------------
// 分类实现
// ---------------------------------------------------------------------------

fn classify_source(
    rec: &Record,
    key: &RecordKey,
    backrefs: &BTreeMap<RecordKey, Vec<RecordKey>>,
) -> (SourceRecordClass, SourceBasis, Vec<SourceBasis>) {
    let deleted = is_deleted_status(rec);
    let mut evidence = Vec::new();

    // 证据按优先级入列：墓碑 → 正式库 → 合并 → 尾块 → PROMOTED_TO → 回指。
    if deleted {
        evidence.push(SourceBasis::StatusDeleted);
    }
    if rec.lib == Lib::Canonical {
        evidence.push(SourceBasis::CanonicalLibrary);
    }
    if rec.lib == Lib::Candidate {
        let merged = rec.get("MERGED_INTO").trim().to_string();
        if !merged.is_empty() {
            evidence.push(SourceBasis::MergedInto(merged));
        }
        if rec.promotion.is_some() {
            evidence.push(SourceBasis::PromotionBlock);
        }
        let promoted_to = rec.get("PROMOTED_TO").trim().to_string();
        if !promoted_to.is_empty() {
            evidence.push(SourceBasis::PromotedTo(promoted_to));
        }
        if let Some(canon) = backrefs.get(key) {
            if !canon.is_empty() {
                evidence.push(SourceBasis::PromotedFrom(canon.clone()));
            }
        }
    }

    let class = if deleted {
        SourceRecordClass::Tombstone
    } else if rec.lib == Lib::Canonical {
        SourceRecordClass::Canonical
    } else if evidence
        .iter()
        .any(|b| matches!(b, SourceBasis::MergedInto(_)))
    {
        SourceRecordClass::CandidateMerged
    } else if evidence.iter().any(|b| {
        matches!(
            b,
            SourceBasis::PromotionBlock
                | SourceBasis::PromotedTo(_)
                | SourceBasis::PromotedFrom(_)
        )
    }) {
        SourceRecordClass::CandidatePromoted
    } else {
        record_unpromoted_marker(&mut evidence);
        SourceRecordClass::CandidateUnpromoted
    };

    let basis = evidence
        .first()
        .cloned()
        .unwrap_or(SourceBasis::NoPromotionMarker);
    (class, basis, evidence)
}

fn record_unpromoted_marker(evidence: &mut Vec<SourceBasis>) {
    if !evidence
        .iter()
        .any(|b| matches!(b, SourceBasis::NoPromotionMarker))
    {
        evidence.push(SourceBasis::NoPromotionMarker);
    }
}

// ---- 目标平面判定 ---------------------------------------------------------
//
// 全部判据只读已解析字段，并使用下面这些显式词表；没有模型临场判断，也没有
// 模糊打分。修改词表就是修改迁移合同，必须同时改测试。

/// 视为“显式硬规则”的 KIND 值（CONSTRAINT 单独处理）。
const RULE_KINDS: &[&str] = &[
    "HARD_RULE",
    "RULE",
    "POLICY",
    "REQUIREMENT",
    "MANDATE",
];

/// KEYS / CONCEPTS 里明确标注为硬规则的词（小写精确匹配）。
const HARD_RULE_TOKENS: &[&str] = &[
    "hard_rule",
    "hard-rule",
    "hard rule",
    "硬规则",
    "硬性规则",
    "硬约束",
    "强制约束",
    "铁律",
    "强制规则",
    "必须遵守",
];

/// 用户画像 / 稳定偏好 / 称呼 / 交互方式 / 环境事实的 KIND。
const PROFILE_KINDS: &[&str] = &[
    "PROFILE",
    "USER_PROFILE",
    "USER_PREFERENCE",
    "PREFERENCE",
    "PREFERENCES",
    "USER_PERSONA",
    "INTERACTION",
    "INTERACTION_STYLE",
    "ENVIRONMENT",
    "ENVIRONMENT_FACT",
    "USER_ENVIRONMENT",
    "ADDRESSING",
    "RELATIONSHIP",
];

/// 画像类 KEYS / CONCEPTS 的精确词表。
const PROFILE_TOKENS: &[&str] = &[
    "profile",
    "user_profile",
    "user-profile",
    "preference",
    "preferences",
    "user_preference",
    "user_persona",
    "addressing",
    "nickname",
    "interaction",
    "interaction_style",
    "communication_style",
    "environment",
    "environment_fact",
    "user_environment",
    "用户画像",
    "用户偏好",
    "偏好",
    "喜好",
    "称呼",
    "称谓",
    "称呼方式",
    "称谓方式",
    "用户称呼",
    "昵称",
    "名字",
    "交互",
    "交互方式",
    "交互偏好",
    "沟通方式",
    "对话方式",
    "环境",
    "环境事实",
    "设备",
    "设备环境",
    "工作目录",
    "语言偏好",
    "语言环境",
];

/// TITLE 的画像短语。只做包含匹配，不做语义判断。
const PROFILE_TITLE_PHRASES: &[&str] = &[
    "用户画像",
    "用户偏好",
    "用户称呼",
    "称呼方式",
    "交互方式",
    "沟通方式",
    "环境事实",
    "用户环境",
];

const SOUL_IDENTITY_KINDS: &[&str] = &[
    "IDENTITY",
    "AI_IDENTITY",
    "ASSISTANT_IDENTITY",
    "SOUL",
    "PERSONA",
    "PERSONALITY",
];
const SOUL_STYLE_KINDS: &[&str] = &["STYLE", "BEHAVIOR", "TONE"];
const SOUL_PRINCIPLE_KINDS: &[&str] = &["PRINCIPLE", "WORK_PRINCIPLE", "VALUES", "VALUE"];

const SOUL_IDENTITY_TOKENS: &[&str] = &[
    "ai_identity",
    "assistant_identity",
    "identity",
    "soul",
    "persona",
    "personality",
    "ai身份",
    "助手身份",
    "人格",
    "人格设定",
    "人设",
    "价值观",
];
const SOUL_STYLE_TOKENS: &[&str] = &[
    "style",
    "tone",
    "回答风格",
    "对话风格",
    "语言风格",
    "表达风格",
    "口语风格",
    "deepseek风格",
    "交流风格",
    "对话语气",
    "语气",
    "人设",
    "风格",
];
const SOUL_PRINCIPLE_TOKENS: &[&str] = &[
    "principle",
    "principles",
    "工作原则",
    "工作准则",
    "行为准则",
    "原则",
    "准则",
];

const SOUL_IDENTITY_TITLE_PHRASES: &[&str] = &[
    "ai身份",
    "助手身份",
    "人格",
    "人设",
];
const SOUL_STYLE_TITLE_PHRASES: &[&str] = &[
    "回答风格",
    "对话风格",
    "语言风格",
    "表达风格",
    "对话语气",
    "语气",
    "风格",
];
const SOUL_PRINCIPLE_TITLE_PHRASES: &[&str] = &[
    "工作原则",
    "工作准则",
    "行为准则",
    "价值观",
];

fn is_deleted_status(rec: &Record) -> bool {
    rec.get("STATUS").trim().eq_ignore_ascii_case("DELETED")
}

fn upper_field(rec: &Record, key: &str) -> String {
    rec.get(key).trim().to_ascii_uppercase()
}

fn lower_field(rec: &Record, key: &str) -> String {
    rec.get(key).trim().to_lowercase()
}

fn field_tokens(rec: &Record, key: &str) -> Vec<String> {
    rec.get(key)
        .split(|c: char| {
            c == ','
                || c == '，'
                || c == ';'
                || c == '；'
                || c == '|'
                || c == '｜'
                || c == '\n'
                || c.is_whitespace()
        })
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

fn fields_have_token(rec: &Record, keys: &[&str], markers: &[&str]) -> bool {
    keys.iter().any(|key| {
        let raw = lower_field(rec, key);
        // 整字段精确命中支持 "hard rule" 这类带空格词；分词后精确命中支持
        // `KEYS=语言偏好,preference` 这类列表。两者都不做子串模糊匹配，
        // 避免 `sharedpreferences` 被误判成 `preference`。
        if markers.iter().any(|m| raw == *m) {
            return true;
        }
        field_tokens(rec, key)
            .iter()
            .any(|token| markers.iter().any(|m| token == m))
    })
}

fn title_has_any(rec: &Record, phrases: &[&str]) -> bool {
    let title = lower_field(rec, "TITLE");
    phrases.iter().any(|p| title.contains(p))
}

fn rule_basis(rec: &Record) -> Option<TargetBasis> {
    let kind = upper_field(rec, "KIND");
    if kind == "CONSTRAINT" {
        let authority = upper_field(rec, "AUTHORITY");
        if authority == "USER_EXPLICIT" || authority == "USER_FEEDBACK" {
            return Some(TargetBasis::KindConstraintUserAuthority);
        }
        // CONSTRAINT 本身即显式硬规则；没有用户 AUTHORITY 也不降级成画像。
        return Some(TargetBasis::ExplicitHardRule);
    }
    if RULE_KINDS.contains(&kind.as_str()) {
        return Some(TargetBasis::ExplicitHardRule);
    }
    if fields_have_token(rec, &["KEYS", "CONCEPTS"], HARD_RULE_TOKENS)
        || title_has_any(rec, HARD_RULE_TOKENS)
    {
        return Some(TargetBasis::ExplicitHardRule);
    }
    None
}

fn profile_signal(rec: &Record) -> bool {
    let kind = upper_field(rec, "KIND");
    if PROFILE_KINDS.contains(&kind.as_str()) {
        return true;
    }
    if fields_have_token(rec, &["KEYS", "CONCEPTS"], PROFILE_TOKENS) {
        return true;
    }
    title_has_any(rec, PROFILE_TITLE_PHRASES)
}

fn soul_signal(rec: &Record) -> Option<TargetBasis> {
    let kind = upper_field(rec, "KIND");
    if SOUL_IDENTITY_KINDS.contains(&kind.as_str()) {
        return Some(TargetBasis::AiIdentitySignal);
    }
    if SOUL_STYLE_KINDS.contains(&kind.as_str()) {
        return Some(TargetBasis::AiStyleSignal);
    }
    if SOUL_PRINCIPLE_KINDS.contains(&kind.as_str()) {
        return Some(TargetBasis::AiWorkPrincipleSignal);
    }

    if fields_have_token(rec, &["KEYS", "CONCEPTS"], SOUL_IDENTITY_TOKENS) {
        return Some(TargetBasis::AiIdentitySignal);
    }
    if fields_have_token(rec, &["KEYS", "CONCEPTS"], SOUL_STYLE_TOKENS) {
        return Some(TargetBasis::AiStyleSignal);
    }
    if fields_have_token(rec, &["KEYS", "CONCEPTS"], SOUL_PRINCIPLE_TOKENS) {
        return Some(TargetBasis::AiWorkPrincipleSignal);
    }

    if title_has_any(rec, SOUL_IDENTITY_TITLE_PHRASES) {
        return Some(TargetBasis::AiIdentitySignal);
    }
    if title_has_any(rec, SOUL_STYLE_TITLE_PHRASES) {
        return Some(TargetBasis::AiStyleSignal);
    }
    if title_has_any(rec, SOUL_PRINCIPLE_TITLE_PHRASES) {
        return Some(TargetBasis::AiWorkPrincipleSignal);
    }
    None
}

fn canonical_episodic_kind(rec: &Record) -> bool {
    if rec.lib != Lib::Canonical {
        return false;
    }
    let kind = upper_field(rec, "KIND");
    matches!(kind.as_str(), "RESULT" | "FAILURE" | "ATTEMPT")
        && !rec.get("SESSION").trim().is_empty()
}

/// 目标平面判定。固定顺序即优先级；每个分支只读字段/词表。
fn classify_target(rec: &Record, source_class: SourceRecordClass) -> (TargetPlane, TargetBasis) {
    if is_deleted_status(rec) {
        return (TargetPlane::Tombstone, TargetBasis::StatusDeleted);
    }
    if let Some(basis) = rule_basis(rec) {
        return (TargetPlane::Rules, basis);
    }
    if profile_signal(rec) {
        return (TargetPlane::Profile, TargetBasis::UserProfileSignal);
    }
    if upper_field(rec, "AUTHORITY") == "USER_EXPLICIT" {
        return (
            TargetPlane::Explicit,
            TargetBasis::AuthorityUserExplicit,
        );
    }
    if let Some(basis) = soul_signal(rec) {
        return (TargetPlane::Soul, basis);
    }
    if source_class == SourceRecordClass::CandidateUnpromoted {
        return (TargetPlane::Episodic, TargetBasis::UnpromotedCandidate);
    }
    if canonical_episodic_kind(rec) {
        return (
            TargetPlane::Episodic,
            TargetBasis::CanonicalEpisodicKindWithSession,
        );
    }
    if rec.lib == Lib::Candidate {
        // 已晋升/已合并候选是溯源对象，不是新的长期知识；保留在 episodic。
        return (
            TargetPlane::Episodic,
            TargetBasis::PromotedCandidateProvenance,
        );
    }
    (
        TargetPlane::LongTerm,
        TargetBasis::CanonicalFallbackLongTerm,
    )
}

// ---------------------------------------------------------------------------
// 冲突实现
// ---------------------------------------------------------------------------

fn build_conflicts(
    entries: &[ManifestEntry],
    records_by_key: &BTreeMap<RecordKey, &Record>,
    old_id_map: &OldIdMap,
    promotion_edges: &[PromotionEdge],
    promotion_issues: Vec<PromotionIssue>,
) -> ConflictReport {
    let mut conflicts = ConflictReport {
        ambiguous_aliases: old_id_map.ambiguous(),
        promotion_issues,
        ..ConflictReport::default()
    };

    // 重复 PROMOTED_FROM：同一值被多条正式记录认领。
    //
    // 这里用 `values` 而不是 `id_values`：旧库的 `PROMOTED_FROM` 有时是来源
    // 文档路径，不是一个可解析的记录身份；即使如此，两条正式记录声称"来自
    // 同一处"仍然需要人工看一眼，不能因为值不像 ID 就静默跳过。
    let mut claims: BTreeMap<String, BTreeSet<RecordKey>> = BTreeMap::new();
    for edge in promotion_edges {
        for value in &edge.values {
            claims
                .entry(value.clone())
                .or_default()
                .insert(edge.canonical.clone());
        }
    }
    for (value, claimed) in claims {
        if claimed.len() < 2 {
            continue;
        }
        let claimed_by: Vec<RecordKey> = claimed.into_iter().collect();
        let mut body_shas = BTreeMap::new();
        for key in &claimed_by {
            if let Some(rec) = records_by_key.get(key) {
                body_shas.insert(key.clone(), body_digest(&rec.body));
            }
        }
        let unique_lines = group_unique_line_count(
            claimed_by
                .iter()
                .filter_map(|k| records_by_key.get(k))
                .map(|r| r.body.as_str())
                .collect(),
        );
        conflicts.duplicate_promoted_from.push(DuplicatePromotedFrom {
            value,
            claimed_by,
            body_shas,
            unique_lines,
        });
    }

    // 正文 SHA 分叉：只看能唯一解析的正式→候选边。
    let mut seen_pairs: BTreeSet<(RecordKey, RecordKey)> = BTreeSet::new();
    for edge in promotion_edges {
        let Some(canonical) = records_by_key.get(&edge.canonical) else {
            continue;
        };
        for candidate_key in &edge.resolved_candidates {
            if !seen_pairs.insert((edge.canonical.clone(), candidate_key.clone())) {
                continue;
            }
            let Some(candidate) = records_by_key.get(candidate_key) else {
                continue;
            };
            let canonical_sha = body_digest(&canonical.body);
            let candidate_sha = body_digest(&candidate.body);
            if canonical_sha == candidate_sha {
                continue;
            }
            let (canonical_only, candidate_only) =
                line_diff_counts(&canonical.body, &candidate.body);
            conflicts.body_sha_divergences.push(BodyShaDivergence {
                canonical: edge.canonical.clone(),
                candidate: candidate_key.clone(),
                canonical_sha256: canonical_sha,
                candidate_sha256: candidate_sha,
                canonical_only_lines: canonical_only,
                candidate_only_lines: candidate_only,
            });
        }
    }
    conflicts
        .body_sha_divergences
        .sort_by(|a, b| a.canonical.cmp(&b.canonical).then(a.candidate.cmp(&b.candidate)));

    // 完全相同的正文 SHA：可能合法副本，也可能重复写入；只列出，不合并。
    let mut by_sha: BTreeMap<String, Vec<RecordKey>> = BTreeMap::new();
    for entry in entries {
        let Some(rec) = records_by_key.get(&entry.key) else {
            continue;
        };
        if rec.body.trim().is_empty() {
            continue;
        }
        by_sha
            .entry(body_digest(&rec.body))
            .or_default()
            .push(entry.key.clone());
    }
    for (sha, mut records) in by_sha {
        if records.len() < 2 {
            continue;
        }
        records.sort();
        let lines = records
            .first()
            .and_then(|k| records_by_key.get(k))
            .map(|r| body_line_count(&r.body))
            .unwrap_or(0);
        conflicts.duplicate_body_sha.push(DuplicateBodySha {
            body_sha256: sha,
            records,
            body_lines: lines,
        });
    }

    conflicts
}

// ---------------------------------------------------------------------------
// 值解析与正文指纹
// ---------------------------------------------------------------------------

/// 把一个引用字段拆成多值。
///
/// 只按分隔符拆（半角/全角逗号分号、换行）；如果整段没有分隔符但由多个
/// "像身份"的词用空格隔开，也拆开。路径和自然语言保持完整，不硬拆。
fn parse_ref_values(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for segment in raw.split([',', '，', ';', '；', '\n']) {
        let token = segment.trim();
        if token.is_empty() {
            continue;
        }
        // 防御：若解析器没把 `PROMOTED_AT=...` 这类下一字段拆开，不要把整段
        // 当成一个引用值。
        if let Some((head, _)) = token.split_once('=') {
            if crate::rec::is_known_key(head.trim()) {
                continue;
            }
        }
        let words: Vec<&str> = token.split_whitespace().collect();
        if words.len() > 1 && words.iter().all(|w| is_ref_shaped(w)) {
            for word in words {
                push_unique(&mut out, word.to_string());
            }
        } else {
            push_unique(&mut out, token.to_string());
        }
    }
    out
}

fn header_id_tokens(rec: &Record) -> Vec<String> {
    let raw = rec.get("ID");
    let mut out = Vec::new();
    for part in raw.split([';', '；', ',', '，']) {
        let mut token = part.trim();
        if let Some(rest) = token.strip_prefix("ID=") { token = rest.trim(); }
        if !token.is_empty() && !token.contains('=') && !token.chars().any(char::is_whitespace) { push_unique(&mut out, token.to_string()); }
    }
    out
}

struct RelationIdentityIndex { by_token: BTreeMap<String, Vec<RecordKey>> }
impl RelationIdentityIndex {
    fn build(records: &[(RecordKey, &Record)]) -> Self {
        let mut by_token: BTreeMap<String, Vec<RecordKey>> = BTreeMap::new();
        for (key, rec) in records {
            let mut tokens = header_id_tokens(rec);
            tokens.push(key.stem.clone());
            for token in tokens { by_token.entry(token).or_default().push(key.clone()); }
        }
        for targets in by_token.values_mut() { targets.sort(); targets.dedup(); }
        Self { by_token }
    }
    fn resolve(&self, value: &str) -> RelationTarget {
        if !is_ref_shaped(value) { return RelationTarget::NonIdentity; }
        match self.by_token.get(value).map(Vec::as_slice).unwrap_or(&[]) {
            [] => RelationTarget::Unresolved,
            [one] => RelationTarget::Resolved(one.clone()),
            many => RelationTarget::Ambiguous(many.to_vec()),
        }
    }
}

pub(crate) fn relation_edges_from_records(records: &[(RecordKey, &Record)]) -> Vec<RelationEdge> {
    let index = RelationIdentityIndex::build(records);
    let mut edges = Vec::new();
    for (key, rec) in records {
        for (ordinal, value) in parse_ref_values(&rec.get("RELATED_IDS")).into_iter().enumerate() {
            edges.push(RelationEdge { source: key.clone(), field: "RELATED_IDS", ordinal, value: value.clone(), target: index.resolve(&value) });
        }
    }
    edges.sort_by(|a, b| a.source.cmp(&b.source).then(a.ordinal.cmp(&b.ordinal)).then(a.value.cmp(&b.value)));
    edges
}
fn is_ref_shaped(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(char::is_whitespace)
        || value
            .chars()
            .any(|c| "，。；：、（）【】《》".contains(c))
    {
        return false;
    }
    if is_short_number(value) {
        return true;
    }
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() < 3 {
        return false;
    }
    if !matches!(parts[0], "C" | "N" | "AB2") {
        return false;
    }
    parts.iter().all(|p| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '_')
    })
}

fn is_short_number(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && (bytes[0] == b'C' || bytes[0] == b'N')
        && bytes[1..].iter().all(|b| b.is_ascii_digit())
}

fn body_digest(body: &str) -> String {
    let normalized = body.replace("\r\n", "\n");
    sha256::digest_hex(normalized.trim().as_bytes())
}

fn body_line_count(body: &str) -> usize {
    body.lines().filter(|l| !l.trim().is_empty()).count()
}

/// 两侧各有几行是对方没有的（非空、去首尾空白后按行集合比较）。
fn line_diff_counts(a: &str, b: &str) -> (usize, usize) {
    let a_lines = line_set(a);
    let b_lines = line_set(b);
    let a_only = a_lines.difference(&b_lines).count();
    let b_only = b_lines.difference(&a_lines).count();
    (a_only, b_only)
}

/// 组内只出现在一条正文里的行数总和（用于重复 PROMOTED_FROM 的 82 行独有统计）。
fn group_unique_line_count(bodies: Vec<&str>) -> usize {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for body in bodies {
        for line in line_set(body) {
            *counts.entry(line).or_insert(0) += 1;
        }
    }
    counts.values().filter(|n| **n == 1).count()
}

fn line_set(body: &str) -> BTreeSet<String> {
    body.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn push_unique(out: &mut Vec<String>, value: String) {
    if !value.is_empty() && !out.contains(&value) {
        out.push(value);
    }
}

fn dedup_sorted<T: Ord>(mut values: Vec<T>) -> Vec<T> {
    values.sort();
    values.dedup();
    values
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn mk(lib: Lib, stem: &str, headers: &str, body: &str) -> Record {
        mk_with_promo(lib, stem, headers, body, None)
    }

    fn mk_with_promo(
        lib: Lib,
        stem: &str,
        headers: &str,
        body: &str,
        promo: Option<&str>,
    ) -> Record {
        let tag = lib.tag();
        let mut raw = format!(
            "[{tag}]\n{headers}\n[/{tag}]\n[MEMORY_BODY]\n{body}\n[/MEMORY_BODY]\n"
        );
        if let Some(p) = promo {
            raw.push_str(&format!("[PROMOTION]\n{p}\n[/PROMOTION]\n"));
        }
        let path = format!("{}/{}.md", lib.dir(), stem);
        Record::parse_str(&raw, Path::new(&path), lib).expect("test record parses")
    }

    fn entry<'a>(m: &'a Manifest, stem: &str) -> &'a ManifestEntry {
        m.entries
            .iter()
            .find(|e| e.key.stem == stem)
            .unwrap_or_else(|| panic!("no entry for {stem}"))
    }

    fn canonical_id() -> &'static str {
        "C-20261101T120000-aaaaaaaa"
    }

    fn candidate_id(n: u8) -> String {
        format!("AB2-N-00{n}")
    }

    #[test]
    fn classification_priority_is_fixed() {
        assert_eq!(
            SOURCE_CLASSIFICATION_PRIORITY,
            [
                SourceRecordClass::Tombstone,
                SourceRecordClass::Canonical,
                SourceRecordClass::CandidateMerged,
                SourceRecordClass::CandidatePromoted,
                SourceRecordClass::CandidateUnpromoted,
            ]
        );
        for (idx, class) in SOURCE_CLASSIFICATION_PRIORITY.iter().enumerate() {
            assert_eq!(class.priority(), idx, "{} moved", class.as_str());
        }
    }

    #[test]
    fn deleted_candidate_is_tombstone_even_with_promotion_block() {
        let rec = mk_with_promo(
            Lib::Candidate,
            &candidate_id(1),
            "ID=AB2-N-001\nTITLE=墓碑\nSTATUS=DELETED",
            "已经删除的正文",
            Some("PROMOTED_AT=2026-10-01"),
        );
        let m = manifest_from_records(vec![rec]);
        let e = entry(&m, &candidate_id(1));
        assert_eq!(e.source_class, SourceRecordClass::Tombstone);
        assert_eq!(e.source_basis, SourceBasis::StatusDeleted);
        assert!(e
            .source_evidence
            .contains(&SourceBasis::PromotionBlock));
    }

    #[test]
    fn deleted_canonical_is_tombstone_not_canonical() {
        let rec = mk(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=旧正式\nSTATUS=DELETED",
            "墓碑正文",
        );
        let m = manifest_from_records(vec![rec]);
        let e = entry(&m, canonical_id());
        assert_eq!(e.source_class, SourceRecordClass::Tombstone);
        assert_eq!(e.source_basis, SourceBasis::StatusDeleted);
        assert_eq!(m.source_class_count(SourceRecordClass::Canonical), 0);
    }

    #[test]
    fn canonical_wins_over_promotion_fields() {
        let rec = mk(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=正式\nPROMOTED_FROM=AB2-N-001\nPROMOTED_TO=C-20261102T120000-bbbbbbbb",
            "正式正文",
        );
        let m = manifest_from_records(vec![rec]);
        let e = entry(&m, canonical_id());
        assert_eq!(e.source_class, SourceRecordClass::Canonical);
        assert_eq!(e.source_basis, SourceBasis::CanonicalLibrary);
    }

    #[test]
    fn merged_candidate_beats_promoted_marker() {
        let rec = mk_with_promo(
            Lib::Candidate,
            &candidate_id(2),
            "ID=AB2-N-002\nTITLE=已并走\nMERGED_INTO=AB2-N-003",
            "并进另一条候选",
            Some("PROMOTED_AT=2026-10-02"),
        );
        let m = manifest_from_records(vec![rec]);
        let e = entry(&m, &candidate_id(2));
        assert_eq!(e.source_class, SourceRecordClass::CandidateMerged);
        assert_eq!(
            e.source_basis,
            SourceBasis::MergedInto("AB2-N-003".to_string())
        );
    }

    #[test]
    fn promotion_block_marks_candidate_promoted() {
        let rec = mk_with_promo(
            Lib::Candidate,
            &candidate_id(3),
            "ID=AB2-N-003\nTITLE=已晋升",
            "候选正文",
            Some("PROMOTED_AT=2026-10-03"),
        );
        let m = manifest_from_records(vec![rec]);
        let e = entry(&m, &candidate_id(3));
        assert_eq!(e.source_class, SourceRecordClass::CandidatePromoted);
        assert_eq!(e.source_basis, SourceBasis::PromotionBlock);
    }

    #[test]
    fn related_ids_keep_resolved_ambiguous_and_non_identity_separate() {
        let source = mk(Lib::Canonical, "C-20261101T120000-aaaa1111", "ID=C-20261101T120000-aaaa1111\nRELATED_IDS=AB2-N-002,candidate/records/AB2-N-002.md", "A");
        let target_a = mk(Lib::Candidate, "AB2-N-002", "ID=AB2-N-002", "B");
        let target_b = mk(Lib::Candidate, "AB2-N-002-copy", "ID=AB2-N-002", "C");
        let manifest = manifest_from_records(vec![source, target_a, target_b]);
        assert_eq!(manifest.relation_edges.len(), 2);
        assert!(matches!(manifest.relation_edges[0].target, RelationTarget::Ambiguous(_)));
        assert!(matches!(manifest.relation_edges[1].target, RelationTarget::NonIdentity));
        assert!(manifest.old_id_map.targets("candidate/records/AB2-N-002.md").len() >= 1);
    }


        let rec = mk(
            Lib::Candidate,
            &candidate_id(4),
            "ID=AB2-N-004\nTITLE=带头部去向\nPROMOTED_TO=C-20261101T120000-aaaaaaaa",
            "候选正文",
        );
        let m = manifest_from_records(vec![rec]);
        let e = entry(&m, &candidate_id(4));
        assert_eq!(e.source_class, SourceRecordClass::CandidatePromoted);
        assert_eq!(
            e.source_basis,
            SourceBasis::PromotedTo("C-20261101T120000-aaaaaaaa".to_string())
        );
    }

    #[test]
    fn plain_candidate_is_unpromoted() {
        let rec = mk(
            Lib::Candidate,
            &candidate_id(5),
            "ID=AB2-N-005\nTITLE=仍未晋升",
            "候选正文",
        );
        let m = manifest_from_records(vec![rec]);
        let e = entry(&m, &candidate_id(5));
        assert_eq!(e.source_class, SourceRecordClass::CandidateUnpromoted);
        assert_eq!(
            e.source_basis,
            SourceBasis::NoPromotionMarker
        );
    }

    #[test]
    fn multi_value_promoted_from_keeps_every_value_and_every_target() {
        let canon = mk(
            Lib::Canonical,
            canonical_id(),
            &format!(
                "ID={canonical_id}\nTITLE=多来源正式\nPROMOTED_FROM=AB2-N-001, AB2-N-002；AB2-N-003",
                canonical_id = canonical_id()
            ),
            "共享正文",
        );
        let mut records = vec![canon];
        for n in 1..=3 {
            records.push(mk(
                Lib::Candidate,
                &candidate_id(n),
                &format!("ID={}\nTITLE=候选{n}", candidate_id(n)),
                "候选正文",
            ));
        }
        let m = manifest_from_records(records);

        let e = entry(&m, canonical_id());
        assert_eq!(m.promotion_edges.len(), 1);
        assert_eq!(
            m.promotion_edges[0].id_values,
            vec!["AB2-N-001", "AB2-N-002", "AB2-N-003"]
        );
        assert_eq!(e.promoted_from_targets.len(), 3);
        for n in 1..=3 {
            let stem = candidate_id(n);
            assert_eq!(m.old_id_map.targets(&stem).len(), 1, "{stem}");
            let c = entry(&m, &stem);
            assert_eq!(c.source_class, SourceRecordClass::CandidatePromoted, "{stem}");
            assert_eq!(c.promoted_by.len(), 1, "{stem}");
            assert_eq!(c.promoted_by[0].stem, canonical_id());
        }
        assert!(m.conflicts.duplicate_promoted_from.is_empty());
        assert!(m.conflicts.promotion_issues.is_empty());
    }

    #[test]
    fn duplicate_promoted_from_is_reported_and_not_collapsed() {
        let canon_a = mk(
            Lib::Canonical,
            "C-20261101T120000-aaaaaaaa",
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=正式A\nPROMOTED_FROM=AB2-N-001",
            "A独有行\n共享行",
        );
        let canon_b = mk(
            Lib::Canonical,
            "C-20261102T120000-bbbbbbbb",
            "ID=C-20261102T120000-bbbbbbbb\nTITLE=正式B\nPROMOTED_FROM=AB2-N-001",
            "B独有行\n共享行",
        );
        let cand = mk(
            Lib::Candidate,
            &candidate_id(1),
            "ID=AB2-N-001\nTITLE=同一个来源",
            "候选正文",
        );
        let m = manifest_from_records(vec![canon_a, canon_b, cand]);

        assert_eq!(m.conflicts.duplicate_promoted_from.len(), 1);
        let group = &m.conflicts.duplicate_promoted_from[0];
        assert_eq!(group.value, "AB2-N-001");
        assert_eq!(group.claimed_by.len(), 2);
        assert!(group.has_body_conflict());
        assert_eq!(group.unique_lines, 2);

        let c = entry(&m, &candidate_id(1));
        assert_eq!(c.source_class, SourceRecordClass::CandidatePromoted);
        assert_eq!(c.promoted_by.len(), 2, "both canonical claims must remain");
    }

    #[test]
    fn ambiguous_old_alias_keeps_all_targets() {
        let a = mk(
            Lib::Candidate,
            "AB2-N-010",
            "ID=AB2-N-009\nTITLE=别名A",
            "A",
        );
        let b = mk(
            Lib::Candidate,
            "AB2-N-011",
            "ID=AB2-N-009\nTITLE=别名B",
            "B",
        );
        let m = manifest_from_records(vec![a, b]);

        let targets = m.old_id_map.targets("AB2-N-009");
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().any(|t| t.key.stem == "AB2-N-010"));
        assert!(targets.iter().any(|t| t.key.stem == "AB2-N-011"));
        assert!(m
            .conflicts
            .ambiguous_aliases
            .iter()
            .any(|a| a.alias == "AB2-N-009"));
    }

    #[test]
    fn body_sha_divergence_keeps_both_sides_and_counts_lines() {
        let canon = mk(
            Lib::Canonical,
            canonical_id(),
            &format!(
                "ID={}\nTITLE=正式\nPROMOTED_FROM=AB2-N-001",
                canonical_id()
            ),
            "alpha\n共享行",
        );
        let cand = mk(
            Lib::Candidate,
            &candidate_id(1),
            "ID=AB2-N-001\nTITLE=候选",
            "beta\n共享行",
        );
        let m = manifest_from_records(vec![canon, cand]);

        assert_eq!(m.conflicts.body_sha_divergences.len(), 1);
        let d = &m.conflicts.body_sha_divergences[0];
        assert_eq!(d.canonical.stem, canonical_id());
        assert_eq!(d.candidate.stem, candidate_id(1));
        assert_ne!(d.canonical_sha256, d.candidate_sha256);
        assert_eq!(d.canonical_only_lines, 1);
        assert_eq!(d.candidate_only_lines, 1);
        assert!(m.conflicts.duplicate_body_sha.is_empty());
    }

    #[test]
    fn identical_body_sha_is_reported_as_duplicate_not_divergence() {
        let a = mk(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=A",
            "同一段正文",
        );
        let b = mk(
            Lib::Candidate,
            &candidate_id(1),
            "ID=AB2-N-001\nTITLE=B",
            "同一段正文\n",
        );
        let m = manifest_from_records(vec![a, b]);

        assert!(m.conflicts.body_sha_divergences.is_empty());
        assert_eq!(m.conflicts.duplicate_body_sha.len(), 1);
        assert_eq!(m.conflicts.duplicate_body_sha[0].records.len(), 2);
    }

    fn target_of(lib: Lib, stem: &str, headers: &str, body: &str) -> (TargetPlane, TargetBasis) {
        let m = manifest_from_records(vec![mk(lib, stem, headers, body)]);
        let e = entry(&m, stem);
        (e.target_plane, e.classification_basis.clone())
    }

    #[test]
    fn target_plane_priority_is_fixed() {
        assert_eq!(
            TARGET_PLANE_PRIORITY,
            [
                TargetPlane::Tombstone,
                TargetPlane::Rules,
                TargetPlane::Profile,
                TargetPlane::Explicit,
                TargetPlane::Soul,
                TargetPlane::Episodic,
                TargetPlane::LongTerm,
            ]
        );
        assert_eq!(
            MEMORY_PLANES,
            [
                TargetPlane::Rules,
                TargetPlane::Profile,
                TargetPlane::Explicit,
                TargetPlane::Soul,
                TargetPlane::Episodic,
                TargetPlane::LongTerm,
            ]
        );
        assert!(!TargetPlane::Tombstone.is_memory_plane());
        assert!(TargetPlane::Rules.is_memory_plane());
    }

    #[test]
    fn target_tombstone_wins_every_other_signal() {
        let (plane, basis) = target_of(
            Lib::Candidate,
            &candidate_id(20),
            "ID=AB2-N-020\nTITLE=墓碑\nSTATUS=DELETED\nKIND=CONSTRAINT\nAUTHORITY=USER_EXPLICIT\nKEYS=profile",
            "墓碑正文",
        );
        assert_eq!(plane, TargetPlane::Tombstone);
        assert_eq!(basis, TargetBasis::StatusDeleted);
    }

    #[test]
    fn target_rules_beats_profile_and_explicit() {
        let (plane, basis) = target_of(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=硬规则\nKIND=CONSTRAINT\nAUTHORITY=USER_EXPLICIT\nKEYS=语言偏好",
            "必须遵守。",
        );
        assert_eq!(plane, TargetPlane::Rules);
        assert_eq!(basis, TargetBasis::KindConstraintUserAuthority);
    }

    #[test]
    fn target_explicit_hard_rule_without_user_authority() {
        let (plane, basis) = target_of(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=技术约束\nKIND=CONSTRAINT\nAUTHORITY=TOOL_EVIDENCE",
            "构建必须离线。",
        );
        assert_eq!(plane, TargetPlane::Rules);
        assert_eq!(basis, TargetBasis::ExplicitHardRule);
    }

    #[test]
    fn target_rules_accepts_user_feedback_and_hard_rule_kind() {
        let (plane, basis) = target_of(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=用户反馈约束\nKIND=CONSTRAINT\nAUTHORITY=USER_FEEDBACK",
            "按反馈执行。",
        );
        assert_eq!(plane, TargetPlane::Rules);
        assert_eq!(basis, TargetBasis::KindConstraintUserAuthority);

        let (plane, basis) = target_of(
            Lib::Canonical,
            "C-20261102T120000-bbbbbbbb",
            "ID=C-20261102T120000-bbbbbbbb\nTITLE=硬规则\nKIND=HARD_RULE\nAUTHORITY=MODEL_DERIVED",
            "硬规则正文。",
        );
        assert_eq!(plane, TargetPlane::Rules);
        assert_eq!(basis, TargetBasis::ExplicitHardRule);
    }

    #[test]
    fn target_profile_beats_explicit_authority() {
        let (plane, basis) = target_of(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=用户偏好\nKIND=PROFILE\nAUTHORITY=USER_EXPLICIT\nKEYS=语言偏好",
            "用户偏好中文。",
        );
        assert_eq!(plane, TargetPlane::Profile);
        assert_eq!(basis, TargetBasis::UserProfileSignal);
    }

    #[test]
    fn target_explicit_authority_without_a_plane_signal() {
        let (plane, basis) = target_of(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=用户交代\nKIND=DECISION\nAUTHORITY=USER_EXPLICIT",
            "按用户交代执行。",
        );
        assert_eq!(plane, TargetPlane::Explicit);
        assert_eq!(basis, TargetBasis::AuthorityUserExplicit);
    }

    #[test]
    fn target_soul_identity_signal() {
        let (plane, basis) = target_of(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=助手身份\nKIND=IDENTITY\nAUTHORITY=MODEL_DERIVED",
            "我是运行在手机上的助手。",
        );
        assert_eq!(plane, TargetPlane::Soul);
        assert_eq!(basis, TargetBasis::AiIdentitySignal);
    }

    #[test]
    fn target_soul_style_signal() {
        let (plane, basis) = target_of(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=回答风格\nKIND=OBSERVATION\nAUTHORITY=MODEL_DERIVED\nKEYS=回答风格",
            "回答要直接。",
        );
        assert_eq!(plane, TargetPlane::Soul);
        assert_eq!(basis, TargetBasis::AiStyleSignal);
    }

    #[test]
    fn target_soul_work_principle_signal() {
        let (plane, basis) = target_of(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=工作原则\nKIND=DECISION\nAUTHORITY=MODEL_DERIVED\nKEYS=工作原则",
            "先弄清事实再动手。",
        );
        assert_eq!(plane, TargetPlane::Soul);
        assert_eq!(basis, TargetBasis::AiWorkPrincipleSignal);
    }

    #[test]
    fn target_unpromoted_candidate_is_episodic() {
        let (plane, basis) = target_of(
            Lib::Candidate,
            &candidate_id(21),
            "ID=AB2-N-021\nTITLE=未晋升候选\nKIND=DECISION",
            "仍待判断。",
        );
        assert_eq!(plane, TargetPlane::Episodic);
        assert_eq!(basis, TargetBasis::UnpromotedCandidate);
    }

    #[test]
    fn target_canonical_result_with_session_is_episodic() {
        let (plane, basis) = target_of(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=结果\nKIND=RESULT\nSESSION=20261101T120000+08:00[Asia/Shanghai]",
            "本轮结果。",
        );
        assert_eq!(plane, TargetPlane::Episodic);
        assert_eq!(basis, TargetBasis::CanonicalEpisodicKindWithSession);
    }

    #[test]
    fn target_canonical_result_without_session_falls_to_long_term() {
        let (plane, basis) = target_of(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=结果\nKIND=RESULT",
            "结果正文。",
        );
        assert_eq!(plane, TargetPlane::LongTerm);
        assert_eq!(basis, TargetBasis::CanonicalFallbackLongTerm);
    }

    #[test]
    fn target_canonical_fallback_is_long_term() {
        let m = manifest_from_records(vec![mk(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=普通决策\nKIND=DECISION",
            "普通正文。",
        )]);
        let e = entry(&m, canonical_id());
        assert_eq!(e.source_basis, SourceBasis::CanonicalLibrary);
        assert_eq!(e.target_plane, TargetPlane::LongTerm);
        assert_eq!(e.classification_basis, TargetBasis::CanonicalFallbackLongTerm);
        assert_eq!(m.target_plane_count(TargetPlane::LongTerm), 1);
    }

    #[test]
    fn target_explicit_hard_rule_from_word_table() {
        let (plane, basis) = target_of(
            Lib::Canonical,
            canonical_id(),
            "ID=C-20261101T120000-aaaaaaaa\nTITLE=规则\nKIND=DECISION\nAUTHORITY=MODEL_DERIVED\nKEYS=硬规则",
            "硬规则正文。",
        );
        assert_eq!(plane, TargetPlane::Rules);
        assert_eq!(basis, TargetBasis::ExplicitHardRule);
    }

    #[test]
    fn target_promoted_candidate_is_provenance_episodic() {
        let rec = mk_with_promo(
            Lib::Candidate,
            &candidate_id(22),
            "ID=AB2-N-022\nTITLE=已晋升候选\nKIND=DECISION",
            "候选正文",
            Some("PROMOTED_AT=20261101"),
        );
        let m = manifest_from_records(vec![rec]);
        let e = entry(&m, &candidate_id(22));
        assert_eq!(e.target_plane, TargetPlane::Episodic);
        assert_eq!(e.classification_basis, TargetBasis::PromotedCandidateProvenance);
    }
}
