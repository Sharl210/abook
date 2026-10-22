# abook 操作总纲（可直接替代提示词中的记忆库章节）

本文件是给模型的完整操作说明。记忆库的一切读写都通过 `abook` 这一个命令完成，
不需要模型自己拼接 aes/awk/sed 命令，也不需要模型记住文件格式。

## 0. 开工前先确认工具可用

```
abook detect
```

输出会说明三件事：当前平台与架构、`abook` 是否已在 PATH 里（能否裸名调用）、
若未安装该下载哪个架构包。**若 detect 报未就绪，先 `abook install` 安装，
不要试图手工找路径。**

约定：
- 记忆库根默认 `~/AgentsBook`，可用 `--root` 或环境变量 `ABOOK_ROOT` 覆盖。
- 所有列表命令都支持 `--page N --per-page N`（默认每页 30，`--per-page 0` 不分页）。
- 所有命令都支持 `--json`，返回机器可读结构。
- 退出码：`0` 成功 / `2` 用法错 / `3` 未找到 / `4` 需批准或被阻断 / `5` 完整性 / `6` IO。

## 1. 读：先看全局，再定位

```
abook state           # 记录数、地图数、收录时刻、一致性一屏看完
abook stats           # KIND / MATURITY / STATUS 分布
abook reconcile       # 目录、地图、名册三方对账（不一致会指出差多少）
abook audit           # 全库格式审计：多少条被纠偏、有没有 ERROR
```

`state` 是一切的起点。**如果 `一致性` 不是"通过"，先 `abook reconcile` 查明差距，
不要带着不一致的库继续写。**

## 2. 找：搜索按代价分五级，按需要选

搜索分五级，级别越高越慢。**默认用 L1，只有 L1 找不到才升级。**

| 级别 | 命令 | 耗时量级 | 什么时候用 |
|---|---|---|---|
| L1 | `abook search 词` | 2ms | 记得原词 |
| L2 | `abook search 词 --fuzzy` / `--regex` | 20ms | 记不准，或要模式匹配 |
| L3 | `abook search 词 --level 3` | 50ms | 只有概念，词不确定 |
| L4 | `abook search 词 --semantic` | 60ms | 想找"换个说法的同一件事" |
| L5 | `abook search 词 --deep` | 130ms | 准确定位不到，要广撒网 |

L1 走字节级预过滤加多线程，不命中的文件根本不解码；L3–L5 在内存里建 BM25 索引，
不落盘、不留任何文件，搜完即弃。

```bash
abook search 保险箱                          # L1
abook search 记忆开关 --fuzzy --distance 1    # L2 模糊
abook search 'GATE[A-Z_]+' --regex           # L2 正则
abook search '^ID=' --regex --raw-lines      # 正则按 KEY=value 整行匹配
abook search 模型不愿执行 --semantic          # L4 语义
abook search 压缩 --field KEYS,TITLE         # 只搜指定字段
abook search 文档 --context 2 --max 5        # 片段带上下文
```

多词默认 AND。`--raw-lines` 用于需要匹配字段名本身的正则（字段值里不含 `ID=`）。

**召回另有专用命令** `abook recall 查询词`：按词面 → READ_WHEN → 分支摘要三级递进，
命中后会自动核对 NOT_FOR 和实体，比 `search` 更贴近"我要不要读这条"的判断。

## 3. 看：单条记录

```bash
abook show <记录>              # 全字段 + 正文
abook show <记录> --page 2 --per-page 50    # 长正文分页
abook get <记录>               # 只列字段名与长度
abook get <记录> KEYS          # 取单个字段
abook cat <记录>               # 只输出正文
abook diff <记录A> <记录B>     # 字段级差异
abook history <关键词>         # 变更历史
abook export --format jsonl --out /tmp/dump.jsonl   # 导出
```

`<记录>` 可以是完整路径、文件名或 ID，三者都能解析。

## 4. 写：写入一律先过校验

**任何写入都会先经容错解析 + schema 校验，不通过则拒绝落盘并报出具体字段。**
写正式库需要 `--approve`；写候选库不需要。

```bash
# 新建候选
abook write --title "标题" --read-when "将来什么情况该读" --body-file /tmp/b.md \
  --kind DECISION --keys "词1,词2" --branch 分支名

# 改一个字段
abook set <记录> MATURITY VERIFIED

# 追加正文（会同步刷新 SUMMARY，但不会覆盖手写摘要）
abook append <记录> --text "补充说明"

# 文本替换
abook patch <记录> --old "旧文本" --new "新文本"
```

**SUMMARY 与 SIZE 的处理规则**：写入时若未提供 SUMMARY，工具从正文首段派生；
此后改正文会**同步更新派生值**，但如果 SUMMARY 是你手写的，工具不会动它。

## 5. 结构操作

```bash
abook promote <候选> --approve     # 候选 → 正式
abook demote <正式> --approve      # 正式 → 候选
abook rename <记录> 新文件名        # 改名（ID 自动对齐）
abook rename <记录> 新标题 --title  # 只改标题
abook move <记录> 目标分支          # 换分支
abook copy <记录> --to canonical   # 跨库复制
abook merge A B C --remove-sources # 合并（可标记源为 SUPERSEDED）
abook split <记录> --at 分隔文本    # 拆成两条
abook delete <记录>                # 软删（可 restore）
abook restore <记录>               # 恢复
```

晋升的授权边界：**只有用户明确批准才能进正式库**。工具会拦，但判断仍需模型自己做。

## 6. 保险箱

```bash
abook vault ls                      # 列条目（标识 + 用途）
abook vault get <标识>              # 取明文
abook vault get <标识> --purpose-only   # 只看用途
abook vault put <标识> --purpose "用途" --stdin
abook vault verify                  # 逐条解密自检
```

写入、更新、删除后工具自动同步名册；名册与目录不相等即判写入未完成。

## 7. 修格式

库是长期演进出来的，历史记录会有各种格式偏差。工具**不靠硬匹配**：

```bash
abook check <记录>          # 只报，不改
abook repair                # 预演：列出要改什么
abook repair --apply        # 真修
abook audit --all           # 看全库纠偏统计
```

容错解析能吸收的情况（全部有真实用例）：

- 一行多个字段（本库 98% 的文件这么写）
- 值里含半角分号但不构成字段（切分时验证后半段确实是合法键名才切）
- 值里含全角分号（全库 2836 次，**从不作分隔符**）
- 缺/多闭合标签（14 个文件）
- 正文里出现 `[MEMORY_BODY]` 字面量（3 个文件）
- 中文键名 `注意=`（30 处）
- 键名别名 `READ-WHEN` → `READ_WHEN`
- BOM、CRLF、无末尾换行
- 字段顺序任意（全库 29 种键序）
- 值跨多行
- 重复字段取首个

**每一次纠偏都会留痕**：`abook audit` 会报出规则号、原文和处置方式，
`abook show <记录> --deviations` 看单条的。容错不等于静默吞掉问题。

## 8. 校验分级

- **ERROR**：记录无法被识别或使用（缺 ID、缺 TITLE）。必须修。
- **WARN**：记录仍可读，但召回质量下降（缺 READ_WHEN、缺 NOT_FOR、KEYS 需从 CONCEPTS 派生）。
  不阻塞使用。

`KEYS` 缺失不算错：早期记录只有 `CONCEPTS`，工具会从 `CONCEPTS` 派生。

## 9. 常见误区

- **不要**手工编辑 `~/AgentsBook` 下的文件。改完地图和索引不会跟着动。
  用 `abook set/patch/append`，工具会同步地图、Index、Global 计数和变更记录。
- **不要**自己算文件名或 ID 哈希。`abook write` 会按规范生成。
- **不要**为了省时间跳过 `state`。不一致的库上做的写入会放大问题。
- **不要**对同一记录并发写。工具没有跨进程锁。

## 10. 一份最简工作流

```bash
abook detect                    # 1. 工具就绪？
abook state                     # 2. 库健康？
abook recall "要做的事"          # 3. 有没有相关记忆？
abook show <命中的记录>          # 4. 读细节
# ... 干活 ...
abook write --title "..." --read-when "..." --body-file /tmp/r.md \
  --kind RESULT --keys "..."    # 5. 记下结果（候选层）
abook audit                     # 6. 确认没写坏
```
