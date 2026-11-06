//! Stable, dependency-free MCP tool registry for the existing abook CLI.
//!
//! This module is intentionally data-only: `main.rs` remains the authoritative
//! route table, while `spec::SPEC` and `spec::POSITIONALS` supply the normal
//! command contracts. Composite commands add their actual nested dispatch
//! contracts here. The MCP layer can serialize `ToolDefinition::to_mcp_value`
//! without constructing a catch-all `argv` parameter.

use crate::mcp_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindingTarget {
    /// Index in the CLI positional vector, after the command/subcommand words.
    Positional(usize),
    /// Emit `--<name> <value>`.
    ValueFlag(String),
    /// Emit `--<name>` only when the supplied JSON value is `true`.
    BooleanFlag(String),
    /// Universal abook library root (`--root <path>`).
    Root,
    /// Fixed command word, normally a composite subcommand.
    Literal(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgumentBinding {
    pub property: String,
    pub target: BindingTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegacyFlagSuppression {
    /// The former all-purpose `argv` escape hatch is never accepted by this API.
    pub legacy_argv_suppressed: bool,
    /// `--json` is supplied by the MCP executor when the CLI command supports it.
    pub inject_json: bool,
    /// Caller-supplied properties that are deliberately not exposed.
    pub suppressed_flags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exposure {
    Public,
    /// Kept as a compatibility descriptor for coverage/transition only.
    /// Must be excluded from tools/list and new MCP calls.
    CompatibilityOnly { reason: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    /// Exact CLI command words, e.g. `attach list`.
    pub command: String,
    /// The first word is an actual `main.rs::routes()` command.
    pub route: String,
    pub description: String,
    pub input_schema: Value,
    pub argument_map: Vec<ArgumentBinding>,
    pub legacy_flag_suppression: LegacyFlagSuppression,
    pub exposure: Exposure,
    /// Canonical command path for aliases; absent for canonical names.
    pub alias_of: Option<String>,
}

impl ToolDefinition {
    pub fn is_public(&self) -> bool {
        matches!(self.exposure, Exposure::Public)
    }

    /// MCP-compatible tool object, with stable abook extension metadata.
    pub fn to_mcp_value(&self) -> Value {
        let bindings = self.argument_map.iter().map(|binding| {
            let target = match &binding.target {
                BindingTarget::Positional(index) => format!("positional:{index}"),
                BindingTarget::ValueFlag(flag) => format!("flag:{flag}"),
                BindingTarget::BooleanFlag(flag) => format!("boolean_flag:{flag}"),
                BindingTarget::Root => "root".to_string(),
                BindingTarget::Literal(value) => format!("literal:{value}"),
            };
            object(vec![
                ("property", Value::s(binding.property.clone())),
                ("target", Value::s(target)),
            ])
        }).collect();
        let suppressed = self.legacy_flag_suppression.suppressed_flags.iter()
            .cloned().map(Value::s).collect();
    let mut metadata = vec![
            ("command", Value::s(self.command.clone())),
            ("route", Value::s(self.route.clone())),
            ("argument-map", Value::arr(bindings)),
            ("legacy-argv-suppressed", Value::b(self.legacy_flag_suppression.legacy_argv_suppressed)),
            ("legacy-json-injected", Value::b(self.legacy_flag_suppression.inject_json)),
            ("suppressed-flags", Value::arr(suppressed)),
        ];
        if let Some(alias) = &self.alias_of {
            metadata.push(("alias-of", Value::s(alias.clone())));
        }
        match &self.exposure {
            Exposure::Public => metadata.push(("exposure", Value::s("public"))),
            Exposure::CompatibilityOnly { reason } => {
                metadata.push(("exposure", Value::s("compatibility-only")));
                metadata.push(("compatibility-reason", Value::s(reason.clone())));
            }
        }
        object(vec![
            ("name", Value::s(self.name.clone())),
            ("description", Value::s(self.description.clone())),
            ("inputSchema", self.input_schema.clone()),
            ("_meta", object(vec![("abook", object(metadata))])),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceDefinition {
    pub uri: &'static str,
    pub name: &'static str,
    pub mime_type: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceTemplateDefinition {
    pub uri_template: &'static str,
    pub name: &'static str,
    pub mime_type: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub text: &'static str,
}

/// Fixed resources currently served by `mcp.rs::resources/list`.
pub const RESOURCES: &[ResourceDefinition] = &[
    ResourceDefinition { uri: "abook://help", name: "abook-help", mime_type: "text/plain", description: "CLI 帮助" },
    ResourceDefinition { uri: "abook://instructions", name: "memory-instructions", mime_type: "text/plain", description: "MCP 使用说明" },
    ResourceDefinition { uri: "abook://state", name: "memory-state", mime_type: "application/json", description: "当前库状态" },
];

/// Fixed resource templates currently served by `mcp.rs`.
pub const RESOURCE_TEMPLATES: &[ResourceTemplateDefinition] = &[
    ResourceTemplateDefinition { uri_template: "abook://skill/{name}", name: "skill", mime_type: "text/markdown", description: "读取指定 Skill" },
    ResourceTemplateDefinition { uri_template: "abook://spec/{name}", name: "spec", mime_type: "text/markdown", description: "读取指定内置规范" },
];

/// Fixed prompt definitions currently served by `mcp.rs::prompts/list` and get.
pub const PROMPTS: &[PromptDefinition] = &[
    PromptDefinition { name: "memory-first", description: "在回答依赖历史信息时先读取记忆", text: "当回答可能依赖历史事实、偏好、项目状态或既有经验时，先调用 memory_context 或 memory_recall，再回答。" },
    PromptDefinition { name: "memory-maintenance", description: "批量预览、应用和核对记忆维护", text: "一次维护多条记录使用 memory_batch_preview 和 memory_batch_apply，不逐条拼接 shell 命令。" },
    PromptDefinition { name: "memory-write", description: "把明确的记忆请求写入指定平面", text: "用户明确要求记住、保存、以后记得、加入画像或作为规则时，在本轮调用 memory_write。" },
    PromptDefinition { name: "skill-authoring", description: "读取 Skill 规范并进行 Skill 工作", text: "先调用 skill_list/skill_read，再读取 skill 规范并执行 Skill 工作。" },
    PromptDefinition { name: "spec-lookup", description: "按主题读取内置规范", text: "需要规则全文时调用 spec_list/spec_get，不在提示词中猜规范。" },
    PromptDefinition { name: "system-update", description: "通过一次工具调用完成系统更新", text: "调用 system_update 完成检查、下载、校验、替换、自检和接管。" },
];

/// All descriptors, including hidden compatibility entries. Sorted by MCP name.
pub fn all_tools() -> Vec<ToolDefinition> {
    let mut out = Vec::new();
    for route in crate::known_commands().iter().copied().filter(|route| !route.starts_with('-')) {
        out.push(top_level_tool(route));
    }
    add_nested_tools(&mut out);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    ensure_unique_names(&mut out);
    out
}

/// Public surface for a modern tools/list response.
pub fn public_tools() -> Vec<ToolDefinition> {
    all_tools().into_iter().filter(ToolDefinition::is_public).collect()
}

/// Lookup includes compatibility records for internal migration and coverage.
pub fn find_tool(name: &str) -> Option<ToolDefinition> {
    all_tools().into_iter().find(|tool| tool.name == name)
}

pub fn build_argv(tool_name: &str, args: &Value) -> Result<Vec<String>, String> {
    let tool = find_tool(tool_name).ok_or_else(|| format!("未知工具: {tool_name}"))?;
    if !tool.is_public() { return Err(format!("工具仅保留兼容用途，未开放 MCP 调用: {tool_name}")); }
    let Value::Object(values) = args else { return Err("tools/call arguments 必须是对象".into()); };
    let Value::Object(schema) = &tool.input_schema else { return Err("工具 schema 不是对象".into()); };
    let allowed = schema.get("properties").and_then(|v| match v { Value::Object(m) => Some(m), _ => None }).ok_or("工具 schema 缺少 properties")?;
    if schema.get("additionalProperties").and_then(|v| match v { Value::Bool(b) => Some(*b), _ => None }) == Some(false) {
        for key in values.keys() { if !allowed.contains_key(key) { return Err(format!("工具参数未声明: {key}")); } }
    }
    let required = schema.get("required").and_then(|v| match v { Value::Array(items) => Some(items), _ => None }).ok_or("工具 schema 缺少 required")?;
    for item in required {
        let Value::String(key) = item else { return Err("工具 schema 的 required 必须是字符串数组".into()); };
        if !values.contains_key(key) { return Err(format!("工具缺少必填参数: {key}")); }
    }
    let mut positional: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut out = tool.command.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    let mut flag_args = Vec::new();
    for binding in &tool.argument_map {
        let Some(value) = values.get(&binding.property) else { continue; };
        match &binding.target {
            BindingTarget::Positional(index) => match value {
                Value::String(s) => { positional.entry(*index).or_default().push(s.clone()); }
                Value::Array(items) => for item in items { match item { Value::String(s) => positional.entry(*index).or_default().push(s.clone()), _ => return Err(format!("参数 {} 的数组元素必须是字符串", binding.property)) } },
                _ => return Err(format!("参数 {} 必须是字符串或字符串数组", binding.property)),
            },
            BindingTarget::ValueFlag(flag) => match value { Value::String(s) => { flag_args.push(format!("--{flag}")); flag_args.push(s.clone()); }, _ => return Err(format!("参数 {} 必须是字符串", binding.property)) },
            BindingTarget::BooleanFlag(flag) => match value { Value::Bool(true) => flag_args.push(format!("--{flag}")), Value::Bool(false) => {}, _ => return Err(format!("参数 {} 必须是布尔值", binding.property)) },
            BindingTarget::Root => match value { Value::String(s) => { flag_args.push("--root".into()); flag_args.push(s.clone()); }, _ => return Err("root 必须是字符串".into()) },
            BindingTarget::Literal(_) => {},
        }
    }
    let positional_route = if tool.command == "memory" {
        positional.remove(&0).unwrap_or_default()
    } else {
        Vec::new()
    };
    if !positional.is_empty() {
        let max = positional.keys().copied().max().unwrap_or(0);
        let mut slots = vec![String::new(); max + 1];
        for (i, values) in positional { if values.len() == 1 { slots[i] = values[0].clone(); } else { slots[i] = values.join("\u{0000}"); } }
        for slot in slots { if slot.contains('\u{0000}') { out.extend(slot.split('\u{0000}').map(str::to_string)); } else if !slot.is_empty() { out.push(slot); } }
    }
    // CLI parsers need the memory subcommand in argv before its flags. Append it
    // ahead of options; then values/booleans cannot be confused with the route.
    if !positional_route.is_empty() { out.extend(positional_route); }
    out.extend(flag_args);
    if tool.legacy_flag_suppression.inject_json { out.push("--json".into()); }
    Ok(out)
}


fn top_level_tool(route: &str) -> ToolDefinition {
    let canonical = crate::spec::canonical(route);
    let row = crate::spec::SPEC.iter().find(|(name, _, _, _, _)| *name == canonical);
    let (params, flags, summary) = match row {
        Some((_, params, flags, summary, _)) => (*params, *flags, *summary),
        None => ("", "", "CLI route without a SPEC row; see command help"),
    };
    let positional_form = crate::spec::positionals(route);
    let positional_form = if positional_form.is_empty() { crate::spec::positionals(&canonical) } else { positional_form };
    let command = route.to_string();
    let name = if route.starts_with('-') {
        format!("abook_route_{}", normalize_tool_segment(route))
    } else {
        format!("abook_{}", normalize_tool_segment(route))
    };
    let (schema, argument_map, legacy_flag_suppression) = build_schema(
        &command,
        positional_form,
        params,
        flags,
        &[],
        false,
    );
    let alias_of = if canonical != route { Some(canonical.clone()) } else { None };
    let exposure = compatibility_exposure(&command);
    let description = if let Some(alias) = &alias_of {
        format!("{summary}（兼容别名：{route} → {alias}）")
    } else if is_composite_root(route) {
        format!("{summary}；对应的复合子命令也以独立 MCP 工具登记")
    } else {
        summary.to_string()
    };
    ToolDefinition {
        name, command, route: route.to_string(), description, input_schema: schema,
        argument_map, legacy_flag_suppression, exposure, alias_of,
    }
}

fn is_composite_root(route: &str) -> bool {
    matches!(route, "attach" | "vault" | "skill" | "branch")
}

fn compatibility_exposure(command: &str) -> Exposure {
    let mut parts = command.split_whitespace();
    let first = parts.next().unwrap_or(command);
    let second = parts.next();
    let lifecycle = matches!(first, "promote" | "demote" | "review" | "staging")
        || (first == "skill"
            && matches!(second, Some("candidate" | "candidates" | "read" | "decide")));
    if lifecycle {
        Exposure::CompatibilityOnly {
            reason: "遗留 candidate/promotion 生命周期路由，仅保留 CLI 兼容与覆盖追踪；不作为 MCP 新 API 暴露。".to_string(),
        }
    } else {
        Exposure::Public
    }
}

#[derive(Clone, Copy)]
struct NestedCommand {
    group: &'static str,
    subcommand: &'static str,
    aliases: &'static [&'static str],
    summary: &'static str,
    positionals: &'static str,
    params: &'static str,
    flags: &'static str,
    /// Explicit extras observed in the concrete handler, not present in SPEC.
    extra_value_args: &'static [&'static str],
    extra_bool_args: &'static [&'static str],
}

const ATTACH_COMMANDS: &[NestedCommand] = &[
    NestedCommand { group: "attach", subcommand: "list", aliases: &["ls"], summary: "列出记录附件", positionals: "<记录>", params: "", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "attach", subcommand: "add", aliases: &["put", "copy"], summary: "把文件加入记录附件", positionals: "<记录> <文件>", params: "name", flags: "force,dry-run,move", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "attach", subcommand: "get", aliases: &["read", "cat"], summary: "取得附件路径", positionals: "<记录> <附件名>", params: "", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "attach", subcommand: "rm", aliases: &["remove", "delete"], summary: "删除记录中的单个附件", positionals: "<记录> <附件名>", params: "", flags: "dry-run,json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "attach", subcommand: "path", aliases: &[], summary: "取得记录附件目录", positionals: "<记录>", params: "", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "attach", subcommand: "sync", aliases: &[], summary: "核对并同步附件清单", positionals: "[记录]", params: "", flags: "all,dry-run,json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "attach", subcommand: "edit", aliases: &[], summary: "查看附件编辑路径和原指纹", positionals: "<记录> <附件名>", params: "", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "attach", subcommand: "commit", aliases: &[], summary: "确认附件编辑并回写清单", positionals: "<记录> <附件名>", params: "", flags: "dry-run,json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "attach", subcommand: "check", aliases: &["orphans"], summary: "列出没有对应记录的附件目录", positionals: "", params: "", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "attach", subcommand: "refs", aliases: &[], summary: "检查正文中的附件引用", positionals: "<记录>", params: "", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "attach", subcommand: "guide", aliases: &["help"], summary: "读取附件命令约定", positionals: "", params: "", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
];

const VAULT_COMMANDS: &[NestedCommand] = &[
    NestedCommand { group: "vault", subcommand: "list", aliases: &["ls"], summary: "列出保险箱条目", positionals: "", params: "", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "vault", subcommand: "get", aliases: &["read", "show"], summary: "读取保险箱条目", positionals: "<标识>", params: "", flags: "purpose-only,json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "vault", subcommand: "put", aliases: &["set", "add"], summary: "写入保险箱条目", positionals: "<标识>", params: "purpose,body,body-file,id", flags: "dry-run,json", extra_value_args: &[], extra_bool_args: &["stdin"] },
    NestedCommand { group: "vault", subcommand: "rm", aliases: &["del", "delete"], summary: "删除保险箱条目", positionals: "<标识>", params: "", flags: "approve,dry-run,json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "vault", subcommand: "ledger", aliases: &[], summary: "读取保险箱名册", positionals: "", params: "", flags: "rebuild,dry-run,json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "vault", subcommand: "repair", aliases: &[], summary: "重建保险箱名册", positionals: "", params: "", flags: "dry-run,json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "vault", subcommand: "verify", aliases: &[], summary: "逐条验证保险箱", positionals: "", params: "", flags: "quiet,json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "vault", subcommand: "names", aliases: &[], summary: "列出保险箱标识与文件名", positionals: "", params: "", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "vault", subcommand: "reconcile", aliases: &["lint"], summary: "对账保险箱文件与名册", positionals: "", params: "", flags: "", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "vault", subcommand: "export", aliases: &[], summary: "导出保险箱条目", positionals: "", params: "out", flags: "dry-run", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "vault", subcommand: "import", aliases: &[], summary: "从文件导入保险箱条目", positionals: "[文件]", params: "from", flags: "dry-run", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "vault", subcommand: "rekey", aliases: &[], summary: "轮换保险箱条目的会话种子", positionals: "", params: "", flags: "dry-run", extra_value_args: &[], extra_bool_args: &[] },
];

const SKILL_COMMANDS: &[NestedCommand] = &[
    NestedCommand { group: "skill", subcommand: "guide", aliases: &["help"], summary: "读取 Skill 约定", positionals: "", params: "", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "skill", subcommand: "path", aliases: &[], summary: "取得 Skill 根目录", positionals: "", params: "root", flags: "", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "skill", subcommand: "list", aliases: &[], summary: "列出 Skill", positionals: "", params: "root", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "skill", subcommand: "show", aliases: &[], summary: "读取 Skill 正文", positionals: "<名>", params: "root", flags: "", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "skill", subcommand: "new", aliases: &[], summary: "创建 Skill 骨架", positionals: "<名>", params: "root,title", flags: "force,dry-run", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "skill", subcommand: "candidate", aliases: &[], summary: "记录 Skill 候选反馈（兼容路由）", positionals: "<名>", params: "root,title,body,body-file", flags: "force,dry-run", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "skill", subcommand: "candidates", aliases: &[], summary: "列出 Skill 候选（兼容路由）", positionals: "<名>", params: "root", flags: "", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "skill", subcommand: "read", aliases: &[], summary: "读取 Skill 候选（兼容路由）", positionals: "<名> <候选>", params: "root", flags: "", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "skill", subcommand: "decide", aliases: &[], summary: "记录 Skill 候选决定（兼容路由）", positionals: "<名> <候选> <决定>", params: "root,reason", flags: "approve,dry-run", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "skill", subcommand: "lint", aliases: &[], summary: "检查 Skill 自包含与导航", positionals: "<名>", params: "root,max-entry", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
];

const TXN_COMMANDS: &[NestedCommand] = &[
    NestedCommand { group: "txn", subcommand: "preview", aliases: &[], summary: "预览批次事务变更", positionals: "", params: "path,body-file", flags: "json,dry-run", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "txn", subcommand: "apply", aliases: &[], summary: "应用批次事务预览", positionals: "", params: "preview-token,expect-revision", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "txn", subcommand: "rollback", aliases: &[], summary: "回滚批次事务", positionals: "", params: "preview-token", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "txn", subcommand: "status", aliases: &[], summary: "读取批次事务恢复状态", positionals: "", params: "", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
];

const MIGRATE_COMMANDS: &[NestedCommand] = &[
    NestedCommand { group: "migrate", subcommand: "scan", aliases: &[], summary: "扫描旧库并返回迁移报告", positionals: "", params: "format", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "migrate", subcommand: "stage", aliases: &[], summary: "构建隔离的六平面迁移暂存树", positionals: "", params: "staging-root", flags: "json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "migrate", subcommand: "activate", aliases: &[], summary: "构建并激活六平面迁移暂存树", positionals: "", params: "staging-root", flags: "activate,json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "migrate", subcommand: "recover", aliases: &[], summary: "恢复中断的迁移激活", positionals: "", params: "", flags: "recover,json", extra_value_args: &[], extra_bool_args: &[] },
    NestedCommand { group: "migrate", subcommand: "rollback", aliases: &[], summary: "回滚迁移并恢复备份库", positionals: "", params: "backup-root", flags: "rollback,json", extra_value_args: &[], extra_bool_args: &[] },
];

const BRANCH_COMMANDS: &[NestedCommand] = &[
    NestedCommand { group: "branch", subcommand: "list", aliases: &["ls"], summary: "列出地图分支", positionals: "", params: "keys,read-when,page,per-page", flags: "json,dry-run,force", extra_value_args: &["lib"], extra_bool_args: &[] },
    NestedCommand { group: "branch", subcommand: "add", aliases: &["new"], summary: "新增地图分支", positionals: "<分支名>", params: "keys,read-when", flags: "json,dry-run,force", extra_value_args: &["lib"], extra_bool_args: &[] },
    NestedCommand { group: "branch", subcommand: "rename", aliases: &["mv"], summary: "重命名地图分支", positionals: "<旧名> <新名>", params: "", flags: "json,dry-run,force", extra_value_args: &["lib"], extra_bool_args: &[] },
    NestedCommand { group: "branch", subcommand: "delete", aliases: &["rm"], summary: "删除地图分支", positionals: "<分支名>", params: "", flags: "json,dry-run,force", extra_value_args: &["lib"], extra_bool_args: &[] },
    NestedCommand { group: "branch", subcommand: "map", aliases: &[], summary: "把记录加入分支", positionals: "<分支> <记录>", params: "", flags: "json,dry-run,force", extra_value_args: &["lib"], extra_bool_args: &[] },
    NestedCommand { group: "branch", subcommand: "unmap", aliases: &[], summary: "把记录移出分支", positionals: "<分支> <记录>", params: "", flags: "json,dry-run,force", extra_value_args: &["lib"], extra_bool_args: &[] },
];

fn add_nested_tools(out: &mut Vec<ToolDefinition>) {
    for command in ATTACH_COMMANDS.iter().chain(VAULT_COMMANDS).chain(SKILL_COMMANDS).chain(BRANCH_COMMANDS).chain(TXN_COMMANDS).chain(MIGRATE_COMMANDS) {
        let mut spellings = Vec::with_capacity(command.aliases.len() + 1);
        spellings.push(command.subcommand);
        spellings.extend_from_slice(command.aliases);
        for spelling in spellings {
            out.push(nested_tool(command, spelling));
        }
    }
}

fn nested_tool(spec: &NestedCommand, spelling: &str) -> ToolDefinition {
    let command = format!("{} {}", spec.group, spelling);
    let route = spec.group.to_string();
    let name = format!("abook_{}_{}", spec.group, normalize_tool_segment(spelling));
    let (mut schema, argument_map, legacy_flag_suppression) = build_schema(
        &command,
        spec.positionals,
        spec.params,
        spec.flags,
        &spec.extra_value_args.iter().map(|x| (*x, false)).chain(spec.extra_bool_args.iter().map(|x| (*x, true))).collect::<Vec<_>>(),
        true,
    );
    if spec.group == "migrate" {
        if let Value::Object(ref mut fields) = schema {
            if let Some(Value::Array(required)) = fields.get_mut("required") {
                match spec.subcommand {
                    "stage" | "activate" => required.push(Value::s("staging_root")),
                    "rollback" => required.push(Value::s("backup_root")),
                    _ => {}
                }
            }
        }
    }
    let alias_of = if spelling == spec.subcommand { None } else {
        Some(format!("abook_{}_{}", spec.group, normalize_tool_segment(spec.subcommand)))
    };
    let exposure = compatibility_exposure(&command);
    let description = if alias_of.is_some() {
        format!("{}（CLI 别名：{spelling}）", spec.summary)
    } else {
        spec.summary.to_string()
    };
    ToolDefinition { name, command, route, description, input_schema: schema, argument_map, legacy_flag_suppression, exposure, alias_of }
}

fn build_schema(
    command: &str,
    positional_form: &str,
    params: &str,
    flags: &str,
    extras: &[(&str, bool)],
    _composite: bool,
) -> (Value, Vec<ArgumentBinding>, LegacyFlagSuppression) {
    let mut properties = BTreeMap::new();
    let mut required = Vec::new();
    let mut bindings = Vec::new();
    let mut suppressed = vec!["argv".to_string()];

    // The tool name and registry metadata already fix the CLI command. Do not
    // expose a second command/subcommand input: requiring it makes valid `{}`
    // calls fail, while accepting a different const would be misleading.

    for (index, (property, is_required, variadic)) in positional_properties(positional_form).into_iter().enumerate() {
        let ty = if variadic { "array" } else { "string" };
        let mut schema = vec![("type", Value::s(ty))];
        if variadic { schema.push(("items", object(vec![("type", Value::s("string"))]))); }
        schema.push(("description", Value::s(format!("CLI 位置参数 {}", index + 1))));
        properties.insert(property.clone(), object(schema));
        if is_required { required.push(Value::s(property.clone())); }
        bindings.push(ArgumentBinding { property, target: BindingTarget::Positional(index) });
    }

    let mut value_names = split_names(params);
    let mut bool_names = split_names(flags);
    for (name, is_bool) in extras {
        if *is_bool { push_unique(&mut bool_names, name); } else { push_unique(&mut value_names, name); }
    }
    // SPEC keeps some switches in both value and flag columns for parser
    // compatibility. MCP must expose one boolean property and one mapping.
    value_names.retain(|name| !bool_names.contains(name));
    // CLI implementations read --root across all normal route families.
    push_unique(&mut value_names, "root");
    let inject_json = bool_names.iter().any(|name| *name == "json");
    if inject_json { suppressed.push("json".to_string()); }

    for name in value_names {
        if name == "json" || name == "argv" { continue; }
        let property = name.replace('-', "_");
        let schema = if name == "root" {
            object(vec![("type", Value::s("string")), ("description", Value::s("覆盖自动定位的 abook 库根。"))])
        } else {
            object(vec![("type", Value::s("string")), ("description", Value::s(format!("CLI 参数 --{name} 的值。")))])
        };
        properties.insert(property.clone(), schema);
        let target = if name == "root" { BindingTarget::Root } else { BindingTarget::ValueFlag(name.to_string()) };
        bindings.push(ArgumentBinding { property, target });
    }
    for name in bool_names {
        if name == "json" || name == "argv" { continue; }
        let property = name.replace('-', "_");
        properties.insert(property.clone(), object(vec![
            ("type", Value::s("boolean")),
            ("description", Value::s(format!("CLI 开关 --{name}。"))),
        ]));
        bindings.push(ArgumentBinding { property, target: BindingTarget::BooleanFlag(name.to_string()) });
    }

    // The `command` property itself selects the command. The field is included
    // for the four CLI routers so callers can see that nested dispatch is fixed.
    let required_values = Value::arr(required);
    let argument_map_value = bindings.iter().map(|binding| {
        let target = match &binding.target {
            BindingTarget::Positional(index) => format!("positional:{index}"),
            BindingTarget::ValueFlag(flag) => format!("--{flag}"),
            BindingTarget::BooleanFlag(flag) => format!("--{flag}"),
            BindingTarget::Root => "--root".to_string(),
            BindingTarget::Literal(value) => format!("literal:{value}"),
        };
        object(vec![("property", Value::s(binding.property.clone())), ("target", Value::s(target))])
    }).collect();
    let schema = object(vec![
        ("type", Value::s("object")),
        ("properties", Value::obj(properties)),
        ("required", required_values),
        ("additionalProperties", Value::b(false)),
        ("x-abook-command", Value::s(command.to_string())),
        ("x-abook-argument-map", Value::arr(argument_map_value)),
        ("x-abook-legacy-flag-suppression", object(vec![
            ("legacy-argv", Value::b(true)),
            ("injected-json", Value::b(inject_json)),
            ("suppressed", Value::arr(suppressed.iter().cloned().map(Value::s).collect())),
        ])),
    ]);
    (schema, bindings, LegacyFlagSuppression { legacy_argv_suppressed: true, inject_json, suppressed_flags: suppressed })
}

fn positional_properties(form: &str) -> Vec<(String, bool, bool)> {
    let mut result = Vec::new();
    for token in form.split_whitespace() {
        if token == "[更多]" || token == "[更多词]" {
            if let Some((name, _, _)) = result.last_mut() {
                let _ = name;
            }
            result.push(("more_terms".to_string(), false, true));
            continue;
        }
        let optional = token.starts_with('[') && token.ends_with(']');
        let inner = token.trim_start_matches('[').trim_end_matches(']');
        let label = inner.trim_start_matches('<').trim_end_matches('>');
        if label.is_empty() || !token.contains('<') { continue; }
        result.push((positional_name(label, result.len()), !optional, false));
    }
    result
}

fn positional_name(label: &str, index: usize) -> String {
    match label {
        "记录" | "正式" => "record".to_string(),
        "记录A" => "record_a".to_string(),
        "记录B" => "record_b".to_string(),
        "子命令" => "subcommand".to_string(),
        "词" => "query".to_string(),
        "字段" => "field".to_string(),
        "值" => "value".to_string(),
        "新名" => "new_name".to_string(),
        "旧名" => "old_name".to_string(),
        "候选" => "candidate".to_string(),
        "决定" => "decision".to_string(),
        "分支" | "分支名" => "branch".to_string(),
        "文件" => "file".to_string(),
        "附件名" => "attachment_name".to_string(),
        "标识" => "ident".to_string(),
        other => {
            let ascii: String = other.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            if ascii.is_empty() { format!("arg_{}", index + 1) } else { ascii.to_ascii_lowercase() }
        }
    }
}

fn split_names(csv: &str) -> Vec<&str> {
    csv.split(',').map(str::trim).filter(|s| !s.is_empty() && *s != "VAULT_SUB").collect()
}

fn push_unique<'a>(items: &mut Vec<&'a str>, value: &'a str) {
    if !items.contains(&value) { items.push(value); }
}

fn normalize_tool_segment(s: &str) -> String {
    let mut out = String::new();
    let mut underscore = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            underscore = false;
        } else if !underscore && !out.is_empty() {
            out.push('_');
            underscore = true;
        }
    }
    while out.ends_with('_') { out.pop(); }
    if out.is_empty() { "command".to_string() } else { out }
}

fn ensure_unique_names(tools: &mut [ToolDefinition]) {
    let mut used = BTreeSet::new();
    for tool in tools {
        let base = tool.name.clone();
        let mut candidate = base.clone();
        let mut suffix = 2usize;
        while !used.insert(candidate.clone()) {
            candidate = format!("{base}_{suffix}");
            suffix += 1;
        }
        tool.name = candidate;
    }
}

fn object(items: Vec<(&str, Value)>) -> Value {
    Value::obj(items.into_iter().map(|(key, value)| (key.to_string(), value)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(tools: &[ToolDefinition]) -> BTreeSet<String> {
        tools.iter().map(|tool| tool.name.clone()).collect()
    }

    #[test]
    fn nested_attach_builds_cli_argv_without_shifting_record_and_file() {
        let args = Value::obj([
            ("record", Value::s("M-debug-abc123")),
            ("file", Value::s("/tmp/source.txt")),
            ("name", Value::s("proof.txt")),
            ("root", Value::s("/tmp/library")),
            ("force", Value::b(true)),
        ].into_iter().map(|(k, v)| (k.to_string(), v)).collect());
        assert_eq!(
            build_argv("abook_attach_add", &args).unwrap(),
            vec![
                "attach", "add", "--name", "proof.txt", "--root", "/tmp/library", "--force",
                "M-debug-abc123", "/tmp/source.txt",
            ],
        );
    }


    #[test]
    fn memory_route_subcommand_is_emitted_before_flags() {
        let args = object(vec![
            ("subcommand", Value::s("write")),
            ("root", Value::s("/tmp/memory")),
            ("plane", Value::s("explicit")),
            ("title", Value::s("T")),
            ("content", Value::s("body")),
        ]);
        let argv = build_argv("abook_memory", &args).expect("build memory route");
        assert_eq!(argv.first().map(String::as_str), Some("memory"));
        assert_eq!(argv.get(1).map(String::as_str), Some("write"));
        let parsed = crate::util::Args::parse(&argv).expect("parse routed memory argv");
        assert_eq!(parsed.pos, vec!["write"]);
        assert_eq!(parsed.get("content"), Some("body"));
    }

        let tools = all_tools();
        let routes: Vec<&str> = crate::known_commands().iter().copied().filter(|route| !route.starts_with('-')).collect();
        for route in &routes {
            let count = tools.iter().filter(|tool| tool.route == *route && tool.command == *route).count();
            assert_eq!(count, 1, "route {route} should have one top-level descriptor");
        }
        let represented: BTreeSet<&str> = tools.iter().filter(|tool| tool.command.split_whitespace().count() == 1)
            .map(|tool| tool.route.as_str()).collect();
        assert_eq!(represented.len(), routes.len(), "all real routes are represented");
    }

    #[test]
    fn special_route_names_are_unique_and_never_collide_with_subcommands() {
        let tools = all_tools();
        assert_eq!(names(&tools).len(), tools.len());
        for route in crate::known_commands().iter().copied().filter(|route| !route.starts_with('-')) {
            assert!(tools.iter().any(|tool| tool.command == route), "missing route {route}");
        }
    }

    #[test]
    fn nested_subcommands_and_aliases_are_all_registered() {
        let tools = all_tools();
        for group in ["attach", "vault", "skill", "branch", "txn", "migrate"] {
            let expected = match group {
                "attach" => ATTACH_COMMANDS,
                "vault" => VAULT_COMMANDS,
                "skill" => SKILL_COMMANDS,
                "branch" => BRANCH_COMMANDS,
                "txn" => TXN_COMMANDS,
                _ => MIGRATE_COMMANDS,
            };
            for item in expected {
                for spelling in std::iter::once(item.subcommand).chain(item.aliases.iter().copied()) {
                    let command = format!("{group} {spelling}");
                    let name = format!("abook_{}_{}", group, normalize_tool_segment(spelling));
                    assert!(tools.iter().any(|tool| tool.command == command && tool.name == name), "missing nested route {group} {spelling}");
                }
            }
        }
        assert_eq!(ATTACH_COMMANDS.len(), 11);
        assert_eq!(VAULT_COMMANDS.len(), 12);
        assert_eq!(SKILL_COMMANDS.len(), 10);
        assert_eq!(BRANCH_COMMANDS.len(), 6);
        assert_eq!(TXN_COMMANDS.len(), 4);
        assert_eq!(MIGRATE_COMMANDS.len(), 5);
    }

    #[test]
    fn names_order_and_schema_command_maps_are_stable_and_unique() {
        let first = all_tools();
        let second = all_tools();
        assert_eq!(first, second);
        assert!(first.windows(2).all(|pair| pair[0].name <= pair[1].name));
        assert_eq!(names(&first).len(), first.len());
        for tool in &first {
            let Value::Object(schema) = &tool.input_schema else { panic!("schema must be object") };
            assert_eq!(schema.get("type"), Some(&Value::s("object")));
            assert_eq!(schema.get("additionalProperties"), Some(&Value::b(false)));
            assert!(schema.contains_key("x-abook-command"));
            assert!(schema.contains_key("x-abook-argument-map"));
            assert!(schema.contains_key("x-abook-legacy-flag-suppression"));
            assert!(tool.legacy_flag_suppression.legacy_argv_suppressed);
            let properties = schema.get("properties").expect("properties");
            assert!(properties.get("argv").is_none());
        }
    }

    #[test]
    fn legacy_candidate_and_promotion_lifecycle_is_hidden_from_public_tools() {
        let all = all_tools();
        let public = public_tools();
        for command in ["promote", "demote", "review", "staging", "skill candidate", "skill candidates", "skill read", "skill decide"] {
            let tool = all.iter().find(|tool| tool.command == command).unwrap_or_else(|| panic!("missing compatibility descriptor {command}"));
            assert!(!tool.is_public(), "{command} must be compatibility-only");
            assert!(tool.to_mcp_value().get("_meta").and_then(|m| m.get("abook")).and_then(|m| m.str("exposure")) == Some("compatibility-only"));
            assert!(!public.iter().any(|candidate| candidate.command == command));
        }
    }

    #[test]
    fn schemas_use_spec_and_positionals_and_suppress_transport_flags() {
        let write = all_tools().into_iter().find(|tool| tool.command == "write").unwrap();
        let properties = write.input_schema.get("properties").unwrap();
        assert!(properties.get("title").is_some(), "SPEC title property");
        assert!(properties.get("body_file").is_some(), "SPEC body-file property");
        assert_eq!(properties.get("json"), None, "json is MCP-managed rather than a user flag");
        assert!(write.legacy_flag_suppression.inject_json);
        assert!(write.legacy_flag_suppression.suppressed_flags.contains(&"json".to_string()));

        let set = all_tools().into_iter().find(|tool| tool.command == "set").unwrap();
        let properties = set.input_schema.get("properties").unwrap();
        for positional in ["record", "field", "value"] {
            assert!(properties.get(positional).is_some(), "POSITIONALS must create {positional}");
        }
    }

    #[test]
    fn fixed_resource_prompt_registry_matches_transport_catalog() {
        assert_eq!(RESOURCES.len(), 3);
        assert_eq!(RESOURCE_TEMPLATES.len(), 2);
        assert_eq!(PROMPTS.len(), 6);
        assert!(RESOURCES.iter().any(|resource| resource.uri == "abook://state"));
        assert!(RESOURCE_TEMPLATES.iter().any(|resource| resource.uri_template == "abook://spec/{name}"));
        assert!(PROMPTS.iter().any(|prompt| prompt.name == "system-update"));
        assert!(PROMPTS.windows(2).all(|pair| pair[0].name < pair[1].name));
    }
}
