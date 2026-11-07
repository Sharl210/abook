# 记忆库操作

记忆库的一切读写都通过 `abook` 一个命令完成。不要自己拼 `awk`/`sed`/`aes` 去操作记忆文件，
也不要记住文件格式——那是工具的事。

## 0. 开工先确认工具可用

```
abook detect
```

它会告诉你：当前平台与架构、`abook` 是否已在 PATH（能否裸名调用）、未装时该下载哪个包。
未就绪就 `abook install`，别手工找路径。

约定：

- 库根默认 `~/AgentsBook`，用 `--root` 或 `ABOOK_ROOT` 覆盖。
- 所有列表命令都分页：`--page N --per-page N`（默认 30，`0` 不分页）。
- 所有命令支持 `--json`。
- 退出码：`0` 成功 / `2` 用法错 / `3` 未找到 / `4` 需批准或受阻 / `5` 完整性 / `6` IO。
- **拼错参数会被直接拒绝**，不会静默忽略。

## 1. 先看全局

```
abook init         # 空库时初始化（含地图、名册、保险箱）
abook state        # 记录数、地图数、收录时刻、一致性，一屏看完
abook stats        # KIND / MATURITY / STATUS 分布
abook reconcile    # 目录 / 地图 / 名册三方对账
abook audit        # 全库格式审计：纠偏多少处、有无 ERROR
abook log          # 变更记录，带大小
abook map          # 地图概览：分支数、路径数、收录时刻
abook atlas [canonical|candidate]      # 逐分支看地图块
abook atlas <库> --card <ID>           # 看某条的地图卡
abook index [关键词]                   # Index.md 的路由表
abook tree         # 分支 → 条目数
abook paths        # 全部路径
abook recent       # 最近变更
```

`state` 是一切的起点。一致性不是"通过"就先 `reconcile` 查明差距，别带着不一致的库继续写。

## 2. 找东西：按代价分五级，默认 L1

| 级别 | 命令 | 量级 | 什么时候用 |
|---|---|---|---|
| L1 | `abook search 词` | 2ms | 记得原词 |
| L2 | `--fuzzy` / `--regex` | 20ms | 记不准，或要模式匹配 |
| L3 | `--level 3` | 50ms | 只有概念，词不确定 |
| L4 | `--semantic` | 60ms | 找"换个说法的同一件事" |
| L5 | `--deep` | 130ms | 准确定位不到，要广撒网 |

L1 走字节预过滤加多线程，不命中的文件根本不解码。L3–L5 在内存里建 BM25 索引，
**不落盘、不留文件、不预处理库**，搜完即弃。

```
abook search 保险箱                          # L1
abook search 记忆开关 --fuzzy --distance 1    # L2 模糊
abook search 'GATE[A-Z_]+' --regex           # L2 正则
abook search '^ID=' --regex --raw-lines      # 正则按 KEY=value 整行匹配
abook search 模型不愿执行 --semantic          # L4 语义
abook search 压缩 --field KEYS,TITLE         # 只搜指定字段
abook search 文档 --context 2 --max 5        # 片段带上下文
```

多词默认 AND。要匹配字段名本身（如 `^ID=`）必须加 `--raw-lines`——普通模式匹配的是字段值。

`abook recall 查询词` 是召回专用：词面 → READ_WHEN → 分支摘要三级递进，命中后自动核对
NOT_FOR 与实体，比 `search` 更贴近"这条要不要读"。

### 上下文与召回

```
abook grep 词 --context 2   # 带前后行，逐行看命中
abook recall "说法"          # 按 KEYS/CONCEPTS 召回候选条目
```

`grep` 适合"我要看原文上下文"，`recall` 适合"我只记得大概怎么说"。

## 3. 看一条记录

```
abook show <记录>                          # 全字段 + 正文
abook show <记录> --page 2 --per-page 50   # 长正文分页
abook show <记录> --field SUMMARY          # 取单个字段
abook get <记录>                           # 只列字段名与长度
abook cat <记录>                           # 只输出正文
abook diff <记录A> <记录B>                 # 字段级差异
abook history [关键词]                     # 变更历史
abook staging [关键词]                     # 待决暂存件（--show 读全文）
abook review [关键词]                      # 晋升记录（--show 读全文）
abook export --format jsonl --out 文件      # 导出
```

`<记录>` 可以是完整路径、文件名或 ID，三者都能解析。

### 列表与分页

```
abook list --lib candidate --per-page 0        # 全部（0 = 不分页）
abook list --q 关键词 --page 2                  # 过滤 + 翻页
abook list --status ACTIVE --maturity VERIFIED
```

所有列表命令共用同一套分页：`--page N --per-page N`（默认 30，`0` 取全部）。
`--limit` / `--size` 会被当成 `--per-page`，`--offset` 当成 `--page`。

## 4. 写入：先过校验

任何写入都先经容错解析加 schema 校验，不通过则拒绝落盘并指出具体字段。
### 旧 CLI 兼容参考

下方示例只用于读取和迁移历史 candidate/canonical 库。新客户端使用前述结构化 MCP 平面工具；不得将 `--approve`/promote 生命周期套用到新 MCP 记忆。

`write` 的实际参数面很短：`--title` `--read-when` 必填，正文用 `--body` 或
`--body-file`，另有 `--not-for` `--lib` `--force` `--dry-run` `--approve` `--json`。
**`--kind` `--keys` `--summary` `--maturity` `--concepts` `--entities` `--project`
`--task` `--session` `--branch` 都不存在**，传了会被逐字拒绝。先拿 `describe` 核一遍：

```
abook describe write
```

正确写法是先落盘、再用 `set` 补头部字段：

```
abook write --title "标题" --read-when "将来什么情况该读" --body-file 文件 --lib candidate
abook set <记录> KIND DECISION
abook set <记录> KEYS "词1,词2"

abook set <记录> MATURITY VERIFIED      # 改一个字段
abook append <记录> --text "补充说明"    # 追加正文
abook patch <记录> --old 旧 --new 新     # 文本替换
```

`set` 支持 KIND / MATURITY / AUTHORITY / SUMMARY / KEYS / CONCEPTS / ENTITIES /
NOT_FOR / EVIDENCE / UNCERTAINTY / SOURCE / RELATED_IDS / SUPERSEDES / PROJECT /
TASK / SESSION 共 16 个字段。`update` 只改 title 与正文。

SUMMARY 与 SIZE：写入时未提供则由正文派生；此后改正文会同步刷新**派生值**，
手写的值不会被覆盖。字段值一律是单行，换行会被折叠成空格。

### 写变更记录与整篇重写

```
abook commit --slug 短名 --body-file F    # 写一条 canonical/commits 记录
abook edit <记录> --body-file F           # 整篇重写正文（--dry-run 先看）
```

`commit` 用于留下"这次改了什么"；`edit` 用于正文需要整体替换而非追加的场合。

### 其余编辑动词

```
abook set    <记录> <字段> <值>    改一个字段
abook unset  <记录> <字段>         删一个字段
abook prepend <记录> --text ...    正文前置
abook update <记录> --title T ...  批量改多字段
abook budget <记录>                读这条要多少字节（对照规则上限）
```

`budget` 在决定"要不要整篇读"之前很有用：先看代价，再决定读不读。

## 5. 新 MCP 记忆入口与兼容命令

新客户端使用结构化 MCP 工具：

```text
memory_write / memory_read / memory_update
memory_delete / memory_restore
memory_search / memory_recall / memory_context / memory_index
memory_batch_preview / memory_batch_apply / memory_batch_status / memory_batch_rollback
memory_export
```

普通记忆平面为 `soul`、`rules`、`profile`、`explicit`、`long_term`、`episodic`；凭据独立存放在 vault，不进入普通记忆导出。`expected_revision` 用于更新、软删除和恢复的冲突保护。

旧 `promote`、`demote`、`candidate`、`canonical` 命令仅保留给旧库迁移和兼容脚本；新 MCP 不使用候选/正式晋升生命周期。

## 8. 新 MCP 与迁移事务入口

新客户端优先使用结构化接口：

```text
abook memory context --root ROOT
abook memory write --root ROOT --plane explicit --title T --content C
abook memory export --root ROOT --plane all
abook migrate --staging-root PATH --verify
abook recover --dry-run
abook txn preview --path file --body-file F
```

## 6. 保险箱

```
abook vault ls                          # 列条目（标识 + 用途）
abook vault get <标识>                  # 取明文
abook vault get <标识> --purpose-only   # 只看用途
abook vault put <标识> --purpose 用途 --stdin
abook vault rm <标识>
abook vault lint                        # 名册与目录对账
abook vault verify                      # 逐条解密自检
```

写入、更新、删除后自动同步名册；名册行数与目录实测条数不等即判写入未完成。

## 7. 修格式

库是长期演进的，历史记录会有各种偏差。工具不靠硬匹配：

```
abook check <记录>        # 只报不改
abook repair              # 预演：列出要改什么
abook repair --apply      # 真修
abook audit --all         # 全库纠偏统计
abook doctor [--fix]      # 健康检查；--fix 自动修可修项
abook fix [记录]          # 同 repair：默认预演，--apply 才写
```

每次纠偏都会记下规则号、原文和处理方式，`show --deviations` 可查。

## 7.5 维护动词

```
abook lint                 # 头部完整性与命名规范（只报）
abook normalize            # 旧命名升为新方案（--dry-run 先看）
abook refresh              # 按磁盘重建地图并推进收录时刻
abook sync                 # 目录 / 地图 / 索引计数对齐
abook install              # 未就绪时自装（按当前架构下载）
abook version              # 版本
```

`normalize` / `refresh` / `sync` 都会改动库，先 `--dry-run`。

## 8. 容错与真实数据的边界

真实库里有这些情况，读取都已覆盖，不要当成错误：

- 字段值跨多行（`SUMMARY` 占 18 行）——续行归前一个字段。
- 一行内嵌多个字段（`ID=xxx;PROJECT=p;TASK=t`）——按 `;` 拆开。
- 容器闭合标签缺失（`[COMMIT]` 开 13 闭 10）——遇下一个同级标记或文件尾即终止。
- `KEYS` 缺失近半——降级用 `CONCEPTS`/`ENTITIES`，不算错误。
- 五种时间格式混用——全部能解析。

**缺失、空值、重复是三种不同状态**，不要当成一件事处理。
