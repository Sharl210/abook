# 记忆库操作

记忆库的一切读写都通过 `abook` 一个命令完成。不要自己拼 `awk`/`sed`/`aes` 去操作记忆文件，
也不要记住文件格式——那是工具的事。

## 0. 优先用 MCP 工具，不是命令

`abook` 本体就是一个 MCP 可执行程序：无参数（或 `abook mcp`）启动即进入 MCP stdio。凡本服务已经
把某项能力暴露成 MCP 工具，任务与执行就走工具调用，**不要退回命令行**。命令行留给工具没覆盖的场合，
以及历史库兼容与迁移。同一件事两者都能做时，工具是默认路径。

如果当前环境还没接入本服务的 MCP，那么拿到可执行文件之后紧接着要做的第一件事就是**为本环境配置它**，
配好再开始干活，别用命令行代跑完事。配置时把库根一并写进启动参数：

```
{"command": "/path/to/abook", "args": ["mcp", "stdio", "--root", "/path/to/AgentsBook"]}
```

这样库定位在配置期就固定，之后每次工具调用都不必再传 `root`，也不会因为运行目录或环境变量变化而指到别的库。
怎么配取决于你在哪个环境——宿主文档、现有配置文件和你自己探查都能给出答案。按当前环境的实际机制配，不必套用别的宿主的写法。

## 1. 开工先确认工具可用

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

## 2. 先看全局

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
abook index                            # 平面计数与收录时刻（六平面）；旧库另支持关键词
abook tree         # 分支 → 条目数
abook paths        # 全部路径
abook recent       # 最近变更
```

`state` 是一切的起点。一致性不是"通过"就先 `reconcile` 查明差距，别带着不一致的库继续写。

## 3. 找东西

六平面库直接用 `memory search`，字段范围由平面决定：

```text
abook memory search --query 保险箱
abook memory search --query 保险箱 --plane explicit
abook memory search --query 保险箱 --field title        # 只搜标题
abook memory search --query 'GATE[A-Z_]+' --regex       # 正则
abook memory grep --pattern 记忆开关 --context 2        # 按行看命中，带前后文
abook memory recall --query 保险箱 --plane long_term
```

维护与追溯：

```text
abook memory list --status deleted      # 回收站
abook memory trash                      # 同上，专用入口
abook memory history --id ID            # 这条为什么变成现在这样
abook memory merge --id A --into B --expect-revision 1 --expect-into-revision 1
abook memory move-plane --id ID --to-plane profile --expect-revision 2
```

下面的五级检索是**旧两库布局**的能力（`--fuzzy` / `--regex` / `--semantic`
都建立在旧记录的字段与分支地图上）。六平面库不认这些参数，传了会明确报错
而不是静默忽略。旧库仍可用，迁移完成后请改用 `memory search`。

### 旧两库布局：按代价分五级，默认 L1

| 级别 | 命令 | 量级 | 什么时候用 |
|---|---|---|---|
| L1 | `abook search 词` | 2ms | 记得原词 |
| L2 | `--fuzzy` / `--regex` | 20ms | 记不准，或要模式匹配 |
| L3 | `--level 3` | 50ms | 只有概念，词不确定 |
| L4 | `--semantic` | 60ms | 找"换个说法的同一件事" |
| L5 | `--deep` | 130ms | 准确定位不到，要广撒网 |

L1 走字节预过滤加多线程，不命中的文件根本不解码。L3–L5 在内存里建 BM25 索引，
**不落盘、不留文件、不预处理库**，搜完即弃。

```text-old-layout
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

## 4. 看一条记录

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

## 5. 写入：先过校验

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

## 6. 新 MCP 记忆入口与兼容命令

新客户端使用结构化 MCP 工具：

```text
memory_write / memory_read / memory_update
memory_delete / memory_restore
memory_search / memory_recall / memory_context / memory_index / memory_outline
memory_batch_preview / memory_batch_apply / memory_batch_status / memory_batch_rollback
memory_export
```

普通记忆平面为 `soul`、`rules`、`profile`、`explicit`、`long_term`、`episodic`；凭据独立存放在 vault，不进入普通记忆导出。`expected_revision` 用于更新、软删除和恢复的冲突保护。

Aurorix 是记忆机制名称；实际命令行工具名是 `abook`。涉及记忆能力时优先使用 abook MCP，相关 MCP 的 `serverName`、服务标签和工具标识应显著包含 `abook`，不要把服务直接命名为 “Aurorix memory”。


写入落位：`soul` 只放 AI 自身稳定身份、风格和行为原则；`profile` 只放用户画像、偏好、称呼和稳定环境事实；`explicit` 只放用户明确说“记住/记下来/你记住”的短小关键点、决策或约定。泛称“沉淀记忆”时，完整经过、证据和可复用经验写入 `long_term`/`episodic`，不默认写入 `explicit`；同一轮既有明确短点又有完整经过时分别写入两个平面。只有用户明确要求记住整个经过或整份对象时，`explicit` 才保存完整内容。

记忆整理是被动 Skill，只能由人类明确调用或外部计划任务明确发出整理指令触发，模型不得因记录变多或常驻层变大而自行整理。触发后分三阶段：拆分常驻层异常大或放错层的记录；梳理下三层的矛盾、重复和废话；从稳定结论提炼并反哺 `soul`/`profile`。整理下三层时同步改标题，标题只概括记忆在讲什么，准确、白话、精简但不让人猜，不替 AI 写用途、适用场景或召回判断。

预载分两条命令：`memory_index` 只完整返回 `soul`、`profile`、`explicit` 正文，每条仅有 `plane`、`title`、`content`；`memory_outline` 只返回 `rules`、`long_term`、`episodic` 的标题，供后续按话题召回。两者分开，避免常驻正文被标题清单撑大后截断。

旧 `promote`、`demote`、`candidate`、`canonical` 命令仅保留给旧库迁移和兼容脚本；新 MCP 不使用候选/正式晋升生命周期。

## 7. 新 MCP 与迁移事务入口

新客户端优先使用结构化接口：

```text
abook memory context --root ROOT
abook memory write --root ROOT --plane explicit --title T --content C
abook memory export --root ROOT --plane all
abook migrate --staging-root PATH --verify
abook recover --dry-run
abook txn preview --path notes/report.txt --body x
```

`txn preview` 与 `memory_batch_preview` 只对 `planes/<平面>/records/` 下的写入做记录格式校验：目标落在记录目录时正文必须能解析成一条记录，否则整库会读不了；写到 `notes/` 这类普通路径不受此限。

## 8. 保险箱

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

## 9. 修格式

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

## 10. 维护动词

```
abook lint                 # 头部完整性与命名规范（只报）
abook normalize            # 旧命名升为新方案（--dry-run 先看）
abook refresh              # 按磁盘重建地图并推进收录时刻
abook sync                 # 目录 / 地图 / 索引计数对齐
abook install              # 未就绪时自装（按当前架构下载）
abook version              # 版本
```

`normalize` / `refresh` / `sync` 都会改动库，先 `--dry-run`。

## 11. 容错与真实数据的边界

真实库里有这些情况，读取都已覆盖，不要当成错误：

- 字段值跨多行（`SUMMARY` 占 18 行）——续行归前一个字段。
- 一行内嵌多个字段（`ID=xxx;PROJECT=p;TASK=t`）——按 `;` 拆开。
- 容器闭合标签缺失（`[COMMIT]` 开 13 闭 10）——遇下一个同级标记或文件尾即终止。
- `KEYS` 缺失近半——降级用 `CONCEPTS`/`ENTITIES`，不算错误。
- 五种时间格式混用——全部能解析。

**缺失、空值、重复是三种不同状态**，不要当成一件事处理。
