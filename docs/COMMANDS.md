# abook 命令参考

单二进制、零依赖。全部命令都接受 `--root <路径>`；不传时库根就是程序自己所处的
目录（其中含 `Global.md`/`Index.md` 时），再退到 `~/AgentsBook`。`ABOOK_ROOT` 仅作
老脚本兼容。

`abook help` 里列了每个命令实际接受哪些参数——写错参数名会被直接拒绝，不会静默忽略。

## 读：了解库

| 命令 | 作用 |
|---|---|
| `state` | 记录数、地图数、收录时刻、最新变更、一致性结论 |
| `stats` | KIND / MATURITY / STATUS 分布；`--page` 分页 |
| `paths` | 打印所有路径 |
| `tree` | 分支树：每条显示 条目数 / 总体积 / 总行数 / 最近活跃；有 `PARENT` 时按层级缩进，`--flat` 平铺 |
| `path` | 反查（`path <记录或分支>`）：给记录 → 列出它所属的分支链（由近及远到顶层）；给分支名 → 向下展开子树。带各级聚合值与时间 |
| `attach` | 附件（文件域）：`list` / `add` / `get` / `rm` / `path` / `sync` / `refs` / `check`；详见下文「附件」 |
| `budget [记录]` | 预载与单条读取的字节预算，对照规则上限 |
| `audit [--all]` | 全库格式审计：纠偏了多少处、有无 ERROR；每条规则都附一句含义 |
| `lint` | 头部完整性与命名规范 |
| `doctor [--fix]` | 健康检查 |
| `refs [--fix]` | 引用完整性：检查 `PROMOTED_FROM`/`RELATED_IDS`/`SUPERSEDES` 等是否指得着；`--fix` 只修能唯一推导的，详见「引用完整性」 |
| `fix` / `repair [记录] [--only 类别]` | 修正可自动修的头部项；默认预演，`--apply` 才写；`--only` 限定类别（`id`/`format`/`enum`/`refs`/`sep`/`trim`/`size`/`keys`），详见「批量修复的范围控制」 |
| `reconcile` | 目录 / 地图 / 名册三方对账 |
| `identity` | Global.md 的凭据身份索引：有什么凭据、能干什么，**不含值** |
| `branch` | 按分支列出条目数与命中键；`--page`、`--json` |
| `recount` `[--apply]` | 用目录实测值核对每个分支的 `COUNT`/`MAXSIZE`；默认预演 |
| `commands` | 全部可输入的命令名，一行一个；`--json`、`--page` |
| `cat` `[记录]` | 只打印正文，不带头部；喂给下游时用它 |
| `describe` `[命令]` | 一条命令的完整调用契约：位置参数、可用选项、是否分页、有无 `--json`；不带参数列全部。`--json` 下 `usage` 字段就是位置参数形状 |
| `leftover` `[--apply]` | 扫描中断写入留下的 `.tmpNNNNN` 残留文件；默认预演 |

## 找：搜索（按代价分级）

| 命令 | 作用 |
|---|---|
| `search <词> [更多词]` | 多词默认 AND；按 `--level` 分级 |
| `recall <查询词>` | 分级召回：词面 → READ_WHEN → 分支摘要 |
| `grep <文本>` | 逐行原文搜索，`--context N` |
| `index [关键词]` | Index.md 路由表 |
| `atlas [lib] [--card 分支]` | 地图分支与卡片字段 |
| `recent` | 最近变更 |
| `log [--page N]` | 变更记录，带大小 |
| `history [关键词]` | 变更记录，`--page` |
| `staging [关键词]` | 待决暂存件列表；`--show <名>` 读全文（归档件只给文件名也能读） |
| `staging resolve <名> --decision 结论` | 结清一件：原文保留、结论追加、移入 `staging/closed/`；`--ref <记录ID>` 挂依据 |
| `review [关键词]` | 晋升记录列表；`--show <名>` 读全文 |
| `map [canonical\|candidate]` | 地图概览：分支数、路径数、收录时刻 |

### 搜索分级

```
L1  ~2ms    确定记得原词            search 词
L2  ~20ms   记不准 / 要模式匹配      search 词 --fuzzy / --regex
L3  ~50ms   只有概念，词不确定       search 词 --level 3
L4  ~60ms   换个说法的同一件事       search 词 --semantic
L5  ~130ms  准确定位不到，广撒网     search 词 --deep
```

L1 只做字面匹配；L2 加正则与编辑距离；L3 起进入 BM25 语义，并用库自身的
`KEYS` / `CONCEPTS` 做扩展；L5 在最宽范围内扩展。级别越高越慢，只有需要时才付这个代价。

`--field F1,F2` 限定字段，`--max N` 限量，`--context N` 带上下文行，
`--lib canonical|candidate|both` 选库，`--include-deleted` / `--hide-promoted` 调过滤。

### 候选记录的四种状态

`list --lib candidate` 的状态不是只读头部那个 `STATUS` 字段，而是从内容推出来的。
原因是晋升与合并都不会改写候选的头部（候选是溯源对象，要保持原样），只留标记：

| 状态 | 判据 | 默认是否列出 |
|---|---|---|
| `ACTIVE` | 没有下面任何标记 | 是 |
| `PROMOTED` | 有 `PROMOTED_TO=` 或末尾 `[PROMOTION]` 块 | 否，`--include-promoted` 才看 |
| `SUPERSEDED` | 有 `MERGED_INTO=`（已被 `merge` 并进另一条） | 否，同上 |
| `DELETED` | 头部 `STATUS=DELETED` | 否，`--include-deleted` 才看 |

已晋升的候选**不是待办**，是溯源副本：正式库里那一条的 `PROMOTED_FROM` 指着它。
默认把它们混进列表，会把一个已经清干净的库读成"还有几百条没处理"。

`index` 的「未晋升候选」段走的也是这套判据，所以它报的数字和
`list --lib candidate` 的默认条数永远一致；两者不一致就是缺陷，不是口径差异。

## 看：单条记录

| 命令 | 作用 |
|---|---|
| `show <记录>` | 全字段 + 正文；`--page` / `--per-page` 分页 |
| `get <记录> [字段]` | 取一个字段；不带字段名则列全部字段名与长度 |
| `cat <记录>` | 只输出正文 |
| `diff <记录A> <记录B>` | 字段级差异 |
| `export [--format jsonl\|json\|md] [--out 文件]` | 导出；`jsonl` 每行一条，`json` 单个数组，`md` 原文 |

`show --fields` 显示完整字段值，`--deviations` 看解析时纠偏过哪些地方。

### 记录选择器

`show` `cat` `get` `update` `delete` `restore` `promote` 等接受四种写法：

- 完整路径
- 库内相对路径 `canonical/records/C-xxx.md`
- 文件名 `C-xxx.md` 或 `C-xxx`
- 记录 ID 或**标题**（标题前缀唯一即可）

多条匹配会报错并列出候选，不会瞎猜。

## 写：改动（正式库需 `--approve`）

| 命令 | 作用 |
|---|---|
| `write` | 新建记录，字段见下 |
| `edit <记录> --body-file F` | 整篇重写正文 |
| `set <记录> <字段> <值>` | 改单个字段 |
| `unset <记录> <字段>` | 删单个字段 |
| `append <记录>` | 追加正文（`--text` / `--file` / stdin） |
| `prepend <记录>` | 前置正文 |
| `patch <记录> --old A --new B` | 文本替换；`--field F` 限定字段，`--all` 替换全部命中 |
| `update <记录> --title T ...` | 批量改字段 |
| `delete <记录>` | 默认软删除（`STATUS=DELETED`），`--hard` 才物理删 |
| `restore <记录>` | 恢复软删除 |
| `rename <记录> <新名>` | 改名；`--title` 同时改标题 |
| `move <记录> <分支>` | 换分支 |
| `copy <记录>` | 复制，`--to canonical\|candidate` |
| `merge <A> <B> ...` | 合并成新记录；源记录标 `SUPERSEDED`，仍可检索 |
| `split <记录> --at <文本>` | 在指定位置拆成两条 |
| `promote <候选>` | 候选 → 正式（需 `--approve`） |
| `promote --all` | 一次晋升全部未归档候选（需 `--approve`，见下） |
| `demote <正式>` | 正式 → 候选（需 `--approve`） |
| `staging resolve <名> --decision 结论` | 结清一件待决件：原文保留、结论追加、移入 `staging/closed/` |
| `commit --slug S --body-file F` | 写一条 `canonical/commits` 变更记录 |

写命令都支持 `--dry-run`，只打印将写入的内容摘要，不落盘。

### `staging resolve`：把一件待决件结掉

待决区是"还等人回答"的队列。回答完之后如果只是知道了、没地方落，文件就只能靠
手工 `mv` 才离开，于是这个数字只增不减——长期不为零的待决数不再提示任何东西，
真正需要人回答的那件就淹在里面。

结清**不是删除**：原文一字不动保留，结论追加在末尾，整件移入 `staging/closed/`。
待决数回答"还有几件在等人"，`closed/` 回答"当初是怎么定的"。删掉原文等于删掉
当时的真实认知，而认知会过期恰恰是当初把它暂存起来的原因。

```sh
abook staging resolve keystore-vault-proposal --decision "保持现状" --ref C-20261001-xxxx
abook staging resolve keystore-vault-proposal --decision "保持现状" --dry-run
```

- `--decision` 必填：缺少就以退出码 2 拒绝。允许"结清但不给结论"等于给了一条
  把问题藏进子目录的快捷方式——数字归零，问题还在。
- `--ref <记录ID>` 可选，把依据记进结清段。
- 只接受 `staging/` 下的一层文件名。带 `..` 或分隔符会被拒：这个命令的用途只是
  移一件待决件，不应能对库外文件动手。
- 结清过的再结一次不报错，把当时记下的结论念出来，**不覆盖**。覆盖会让两条结论
  叠在一起，读者不知道该信哪条。
- 两边同名文件同时存在时报完整性错误（退出码 5），不替你挑一份：那说明上次只做了
  一半，覆盖会丢结论，删原件会丢原文。
- 归档件仍可读：`abook staging --show <名>` 只给文件名就会落到 `closed/` 里找。

### `promote --all`：批量晋升

单条 `promote` 每跑一次都要重写地图、索引、全局计数与变更日志，四个文件加起来
两百多 KB，几十条积压就是几十次全量重写，而且每条都是独立事务——中途失败会留下
半批状态。`--all` 把它变成一次事务：记录各自原子落盘，地图与索引各写一次，
变更日志只写一条汇总。

```sh
abook promote --all --dry-run          # 先看会做什么，列出真实目标文件名
abook promote --all --approve          # 真跑
abook promote --all --kind LESSON --q 缓存 --approve   # 只处理匹配的一批
```

分批过滤用 `--kind` / `--maturity` / `--q`（匹配标题、KEYS、正文）/ `--branch`。

“已归档”与“跳过”是两回事，输出里分开报：

- **已归档**：早就晋升过（头部 `PROMOTED_TO=` 或末尾 `[PROMOTION]` 块），
  或已被 `merge` 合并进另一条（`MERGED_INTO=`）。它们没有失败，不该算进这一批的账。
- **跳过**：已软删，处理不了，需要人看为什么。

有失败时退出码为 5，调用方据此判断这批清没清干净。

写命令也都支持 `--json`。成功时给一个对象，`changed` 列出本轮真正写过的文件；
失败时同样给对象，`error.tag` 是 `USAGE` / `BLOCKED` / `NOT_FOUND` / `INTEGRITY` /
`IO` / `ERROR` 之一，`error.exit` 就是进程退出码。这样调用方不必去解析那句中文。

不传 `--json` 时输出是给人看的短句；无论哪种情况，退出码含义相同。

### write 的字段

必填：`--title` `--read-when`，正文用 `--body` 或 `--body-file`。

其余全部可选，完整清单就是 `abook describe write` 列出的那些：
`--not-for` `--body-file` `--lib` `--approve` `--dry-run` `--force` `--body` `--json`。

**注意：`write` 不接受 `--kind` `--maturity` `--authority` `--summary` `--keys`
`--concepts` `--entities` `--evidence` `--uncertainty` `--source` `--related`
`--supersedes` `--branch` `--project` `--task` `--session`。** 传了会被逐字拒绝
（退出码 2）。要写这些字段，先 `write` 落盘拿到记录名，再用 `set` 逐个补：

```sh
abook write --title "标题" --read-when "何时该读" --body-file 正文.md --lib candidate
abook set N-20261024T061326-abc12345 KIND DECISION
abook set N-20261024T061326-abc12345 KEYS "词1,词2"
```

`set` 实测支持上述 16 个字段（KIND / MATURITY / AUTHORITY / SUMMARY / KEYS /
CONCEPTS / ENTITIES / NOT_FOR / EVIDENCE / UNCERTAINTY / SOURCE / RELATED_IDS /
SUPERSEDES / PROJECT / TASK / SESSION）。`update` 只能改 title 与正文，补头部字段用 `set`。

### 一次写入会同步更新

1. `records/` 下的记录文件（临时文件 + 复读校验 + 原子改名）
2. 该库的 `Atlas.md`（分支 PATH / COUNT / MAXSIZE / REVISION / 收录时刻）
3. `Index.md` 的路由行
4. `Global.md` 的记录数与两图收录时刻
5. `commits/` 下一条变更记录

内容没变的文件不会被重写；命令结束时会把本轮真正改动过的文件列出来。
任一步失败都保留最近有效版本并返回非零退出码。

### 附件（文件域）

一条记录 = 文字域 + 文件域。

```
records/C-20261024T124407-d4033a24.md          ← 文字域：有字节，也有行数
records/C-20261024T124407-d4033a24-refs/       ← 文件域：有字节，没有"行"
    01-screenshot.png
    raw-capture.log
```

放进 `-refs/` 的任何东西都是附件：截图、录屏、原始日志、PDF、抓包、表格。
**纯文本也算附件**——只要不是记录本体，它就是这条记录携带的证据。

目录名规则 `<条目名>-refs`，与 Skill 的引用文件夹同构。读取时也认
`-ref` / `-references` / `-assets` / `-files`，手建的目录不会被忽略；
多个命名并存时**全部统计**。

#### 命令

| 命令 | 作用 |
|---|---|
| `attach list <记录>` | 列出附件：名字 + 体积；`--json` 给结构化 |
| `attach add <记录> <文件>` | 复制进附件目录；`--name` 改名，`--move` 搬走，`--force` 覆盖/超限确认 |
| `attach get <记录> <附件名>` | **打印绝对路径**，不打印内容 |
| `attach rm <记录> <附件名>` | 删一个附件；`--dry-run` 预演 |
| `attach path <记录>` | 打印附件目录路径 |
| `attach edit <记录> <附件名>` | 取路径 + 当前指纹，进入编辑 |
| `attach commit <记录> <附件名>` | 确认改动，回写清单并刷新 `UPDATED` |
| `attach sync <记录>` | 清单与磁盘重新对齐；`--all` 全库，`--dry-run` 只报 |
| `attach refs <记录>` | 列出正文里的附件引用，标出悬空的 |
| `attach check` | 列出没有对应记录的孤立附件目录 |

`attach` 不带子命令、或 `attach guide`，会打印这套约定。

#### 为什么只返回路径

附件可能是 40 MB 视频。把字节塞进输出从来不是想要的：模型拿到路径就能自己
去读、去解析，工具不该替它搬一遍。

#### 清单：`[ATTACH]` 块

每次附件变动，工具会把当前清单回写进记录末尾：

```
[ATTACH]
1	N-20261024T162534-3be3b0df-refs/a.log	4
2	N-20261024T162534-3be3b0df-refs/b.txt	4
[/ATTACH]
```

- 每行 `序号<TAB>相对名<TAB>字节<TAB>指纹`。用 TAB 是因为文件名可以含空格。
- 指纹是 FNV-1a 64（16 位十六进制），用来发现「字节数没变但内容被改了」。
- **三列老格式照常读**：判据是"最后一段像不像 16 位十六进制"，不是"一共几段"。
  老记录第一次 `sync` 时自动补上指纹。
- 序号供正文引用：`本条目附件1://a.log` 指第 1 行。
- 回写走正常的记录写盘路径，所以 `UPDATED` 会跟着刷新——**附件变了，条目就算
  改过**，列出时自然排到前面。
- 清单是记录的**派生部分**，不走 `--approve` 批准门：附件增删已经是执行完的
  确定事实。
- 记录自己因此能说清"我带了什么"，不必先列目录。

#### 改附件：两步协议

附件正文在文件系统里，模型通过路径去改是唯一现实的做法——把内容搬进命令
输出再搬回去既慢又会在二进制上出错。但**给了路径，工具就看不见改动发生的
那一刻**：文件系统不会通知任何人。

所以做成两步：

```
abook attach edit <记录> note.txt      # 打印路径 + 改之前的指纹
#   模型按路径去改
abook attach commit <记录> note.txt    # 比对指纹，确认变了才回写
```

`commit` 只接受"内容确实变了"：没变就直说没变，不制造空变更记录。

**漏了 commit 不会丢数据。** 清单里的指纹会与磁盘对不上，`attach list` /
`attach get` / `attach sync` 每次接触这条记录都会把它报出来。协议是让人少
犯错，不是让人犯错就丢数据。

#### 何时发现：接触即检测

改动的那一刻工具不知道，**下一次任何命令接触这条记录时才知道**。这是路径
模式的固有代价，所以把检测挂在每一次接触上，让"漂移存在过但没人知道"的
窗口尽量短。

四类差异：

| 差异 | 含义 |
|---|---|
| 清单有磁盘无 | 附件被删了（或绕过工具 `rm` 了） |
| 磁盘有清单无 | 附件被加进来但没走 `add` |
| 字节数变了 | 内容被替换 |
| **内容被改过** | 字节数**没变**但指纹对不上 |

最后一类最危险也最容易被漏掉：把附件里的 `AAAA` 改成 `BBBB`，大小一字不差，
只比字节数会报"一切正常"，而清单指着的内容已经换了个样。所以清单第四列存
FNV-1a 64 指纹，比较的是内容不是长度。

超过 4 MB 的附件按「头 64KB + 尾 64KB + 长度」抽样——没人会只改一个 50 MB
文件的第 20 MB 处还指望不被察觉，而那种情况本来就该重新 `add`。

#### 其他一致性命令

- `attach refs` 检查正文引用指向的第 N 个附件是否存在，悬空的会报出来。
- `attach check` 找孤立目录（记录已改名/删除，附件还在），**只报告不删除**：
  因为记录改名就删掉别人的截图不是修复。

#### 旧库兼容

v3 之前的库没有任何附件目录、记录里也没有 `[ATTACH]` 块。这类库：

- 读取完全正常，`attach list` 报「没有附件」。
- `attach sync --all` 报 0 条漂移，不会凭空写清单。
- 只有真的加了附件才会出现 `[ATTACH]` 块——**改造是逐条的、懒惰的**，
  不做全库迁移。

#### 复制效率

`attach add` **默认复制，不移动**：只有用户明确说"搬过去"时才加 `--move`。
默认移动会让源文件凭空消失——调用方说的是"加进附件目录"，不是"从原处拿走"。

| 情况 | 走哪条 | 代价 |
|---|---|---|
| 默认 | 复制 | 源文件保留 |
| `--move` 且同设备 | `rename` | 零拷贝，瞬间（源文件消失） |
| 支持 CoW 的文件系统 | reflink（`fs::copy` 内部） | 共享数据块，不占双倍 |
| 跨设备 / 网络路径 / WSL `/mnt/c` / UNC | 内核级复制 | `copy_file_range` / `CopyFileExW` / `fcopyfile` |

不自己写读写循环：手写缓冲在网络路径上比内核实现慢一个数量级——内核知道
自己在跟 SMB 讲话，知道该用多大的块。

超过 50 MB 需要用户明确同意，加 `--force` 表示已获授权。**没有免检通道**：
这条线管的是记忆库会长多大，同盘复制、`--move` 搬入同样占空间，reflink 也只是
推迟实际占用。记忆库记的是记忆，大素材本身就不是它的主营。

用户已讲过「你决定」「全权由你」一类的话，那句就是授权，直接加 `--force`
继续，不必为每份文件再问一遍。

复制失败时会说明卡在哪一步、接着该怎么做（例如源文件读不了时提示先复制到
有权限的位置再重试），并清掉半截文件——半份附件比没有更坏。

### 时间：`UPDATED` 字段

每次改动记录都会在头部写入 `UPDATED=<本地时间含时区偏移>`，例如
`2026-10-24T15:42:07+08:00[Asia/Shanghai]`。这是记录自己携带的时间，因此
复制目录、解压归档、从美国搬到中国这类**会重置所有文件 mtime 的操作都不影响它**。

- 存储保留完整偏移，展示一律换算到**当前设备时区**；换个时区打开同一个库，
  显示的时间会跟着换算，数值不变。
- **旧库兼容**：本轮之前写的记录没有这个字段。读取时按三源回退取时间
  （声明字段 → 文件名里的时间戳 → mtime），所以老记录照常显示，不需要迁移。
  一条记录在**第一次被编辑时**才懒补上 `UPDATED`，不做全库批量改写。
- 排序口径：列出类命令（`list` / `tree` / `atlas` / `staging` / `review` / `history`）
  按**最后改动时间降序**——改过就说明最近被想着，排前面。
  `search` / `recall` 仍按**匹配度降序**，时间只作展示字段，不参与排序。

### 体积与行数：为什么每个条目都显示

`list` / `search` / `recall` / `show` / `get` / `tree` / `atlas` / `path` 的输出都带
每条记录的体积与行数。理由是**读之前要先能判断代价**：

- 只知道"命中 20 条"，不知道其中一条 120KB，就会一次性把上下文撑爆；
- 反过来，知道某条只有 400B / 12 行，就能放心整篇读；
- 行数是给"翻到第 40 行看看"这种定位用的。

`tree` / `atlas` / `path` 的分支行给的是**聚合值**：该分支管辖的条目数、总体积、
总行数。聚合按分支 PATH 去重后求和，所以条目数与体积、行数永远对得上。

### 分页：默认 30KB 预算

`show` / `cat` 默认只输出**前 30KB**，超了在末尾明确写"已截断"并给出三个出路：

```
第 1/4 页  行 1-308  本页 30.0KB  全文 117.3KB 120092  1200 行  下一页 --page 2  全量 --all  改预算 --kb <KB>
```

| 想要 | 加什么 |
|---|---|
| 下一页 | `--page 2` |
| 一次全看 | `--all` |
| 换预算 | `--kb 10`（10KB）、`--kb 100` |
| 按行分页 | `--per-page 40`（沿用旧行为） |

切分点总落在**行边界**且对齐字符边界，所以不会出现半行或半个汉字；
代价是每页可能略小于预算，不会超过。

### 层级：分支可以挂在分支下

Atlas 卡片支持两个可选字段：

```
BRANCH=记忆机制细节
PARENT=记忆系统          # 父分支的 BRANCH 名，不是章节标题
LEVEL=2                  # 仅参考；真实深度由 PARENT 链计算
```

两处都不写就是顶层分支——**现有地图全部无需改动，行为与以前完全一致**。

- `tree` 检测到有 `PARENT` 才切成树形缩进（`▸` 顶层 / `·` 下级），兄弟按体积降序；
  `--flat` 强制平铺。
- `path <记录>` 由记录**向上溯源**到顶层；`path <分支>` 由分支**向下展开**子树。
  父子链按 PARENT 走，能发现"父分支名不存在"的悬挂引用并如实报出。
  PARENT 成环会停在环上并标注，不会死循环。

## 修：格式问题

| 命令 | 作用 |
|---|---|
| `check [记录\|-]` | 检查一条（或 stdin），只报不改 |
| `refs [--fix] [--approve] [--json]` | 引用完整性检查；`--fix` 预演，加 `--approve` 执行 |
| `repair [记录] [--apply] [--only 类别]` | 按校验器自动修复，默认预演；`--only` 限定类别 |
| `fix [记录]` | 同 `repair` |
| `normalize [lib] [--dry-run]` | 旧命名升级为 `前缀-时间戳-哈希8` |
| `refresh [lib]` | 按磁盘重建地图路径并推进收录时刻 |
| `sync` | 同步 Global 计数与地图收录时刻 |

`refresh` 是幂等的：路径集合没变时不会推动 `REVISION`，也不会重排已有顺序。

输出报的是**刷新后地图里一共有多少条路径**，以及本次新增了几条：

```
canonical/Atlas.md: 本次新增 0 条，地图现有 445 条路径
```

只报"本次新增"会误导：地图已经完整时它恒为 `0 条路径`，看起来像什么都没做，
实际可能是刚把几十条缺失路径补齐。判断地图对不对，看的是"现有"那个数字，
它必须等于 `state` 里的「地图 N 条」。

## 保险箱

| 命令 | 作用 |
|---|---|
| `vault ls` | 列条目（只解密首行，取标识与用途） |
| `vault get <标识>` | 取明文；`--purpose-only` 只给用途 |
| `vault put <标识> --purpose P` | 写入；正文用 `--body` / `--body-file` / `--stdin` |
| `vault rm <标识>` | 删除条目 |
| `vault ledger [--rebuild]` | 看或重建名册 |
| `vault export --out F` | 导出全部明文 |
| `vault import --in F` | 从导出文件导入 |
| `vault rekey` | 换会话种子，明文不变 |
| `vault verify` | 逐条解密自检 + 文件名派生校验 |
| `vault names` | 标识 → 派生文件名（只列名，不解密） |
| `vault lint` | 名册与目录对账 |
| `vault repair` | 重建名册 |
| `vault reconcile` | 文件数与名册行数对账 |

别名：`list` `export` 之外的 `ledger` / `names` / `reconcile` 与上面同名项等价。

写入后自动解密回读比对，不一致直接判失败，不会把坏条目当成功。

## 规范外挂：spec

提示词里保留每个规范的入口行，正文放在二进制里。这样提示词只带骨架，读全文时才付字节代价，
而且读到的就是原文本身，不是模型的复述。

| 命令 | 作用 |
|---|---|
| `spec` | 列目录：每个规范多少字节、讲什么 |
| `spec <名>` | 取该规范全文（与提示词里曾内联的版本逐字相同） |
| `spec list` | 只列名字，每行一个 |
| `spec --json` / `spec <名> --json` | 机器可读 |

可用名字共 12 份：`plan` `ulw` `security` `compaction` `maintenance` `magic` `skill`
`promote` `attach` `vault` `stopbudget` `ui`。

一次只取当前要用的那一个。12 份合计 49KB，全部读一遍会白占上下文；`spec` 不带名字时
给出的正是「哪个规范讲什么」，用来选名，不是用来代替正文。

**这 12 份是怎么选出来的。** 判据三条，缺一不可：体积够大、只在特定事件发生时才需要、
**抽走之后调用方仍然知道它存在**（提示词里留得下门名清单）。

第三条是关键。如果只把正文搬进工具、提示词里一个名字都不留，调用方不会去取——它不知道
自己缺什么。所以骨架里把门名逐一列出，那是入口，不是装饰。

有三类**故意不抽**：

- **故障恢复类**（冷启动步骤、重载流程）。价值恰好出现在"其他东西都坏了"的时候，
  依赖工具存在就失去意义。
- **每轮都要用的**（首显、路由、交付状态）。抽走之后每轮都要多一次往返，更贵。
- **边界类**（发布产物净化、禁拒）。抽走正好会在最该看见它的时刻看不见。

**符号归属**：骨架里列出的每个名字，其正文定义都在对应的 `spec <名>` 里。新增或改名时
两处同改，只改一处就是悬空引用。验证方式是骨架里的名字去 `spec` 输出里找——找到就对了，
因为验证和执行读的是同一份数据。

## Skill 约定与反哺：skill

| 命令 | 作用 |
|---|---|
| `skill` / `skill guide` | 打印五节约定全文（`--json` 给机器读） |
| `skill path` | Skill 根目录 |
| `skill list` | 已有哪些 Skill：入口行数、引用文件数、候选数，超预算会标出来 |
| `skill show <名>` | 该 Skill 的 SKILL.md 与候选概况 |
| `skill new <名>` | 建骨架（SKILL.md + candidates/ + references/，入口自带预算提示与导航表） |
| `skill candidate <名> --title T --body-file F` | 写一条反哺候选 |
| `skill candidates <名>` | 列该 Skill 的候选 |
| `skill read <名> [序号]` | 读候选全文 |
| `skill decide <名> <序号> accept\|reject [--reason R]` | 记录决定，写 decisions.log |
| `skill lint <名>` | 实证检查：自包含 + 主入口预算 + 导航可达（`--max-entry N` 覆盖 120 行预算） |

`lint` 查四类，任一项不过就退出码 5：外部路径与跨 Skill 引用；主入口超预算；
引用文件没在主入口登记（运行时发现不了它，等于不存在）；主入口登记了却不存在的
悬空路径。`candidates/` 不参与扫描——待决候选还没被采纳，里面的路径和日志是证据，
不是运行时依赖。`docs/`、`assets/` 这类如果在本 Skill 里没有同名目录，按"用户项目
的路径"处理，不算悬空。

为什么主入口要有预算：它是每次调用都整篇进上下文的那个文件，体积是**每次**的成本；
引用文件的体积只是**用到时**的成本。两者不该同等对待。

候选落在 `<技能根>/<名>/candidates/`，与记忆库同级但分开——两边是两回事，
不共用目录、不互相引用。技能根不是写死的：工具在常见位置里挑"装得最多"的那个
（`~/.dsh/skills`、`~/.agents/skills`、`~/.config/agents/skills`、`~/AgentSkills`），
因为"第一个存在的"往往是一两份实验残留，而 `list` 报"共 1 个"看起来完全正常，
没人会怀疑路径挑错了。

**`skill decide accept` 只记决定，不动 SKILL.md。** 这里没有机械晋升：把候选合并进
SKILL.md 由模型读完全部候选后自己改写完成。`lint` 只查形式，判断「哪几条该并、怎么并」
不在工具能力内。

## 别名

同一件事的另一种写法。别名和正式名完全等价，`describe` 会标出它属于哪条命令。

唯一的例外是 `cat`：它和 `show` 走同一条解析路径，但只打印正文，不打印头部。
把记录喂给下游时用它，省掉再剥一层字段。

| 别名 | 等价于 |
|---|---|
| `ls` | `list` |
| `add` | `write` |
| `edit` | `update` |
| `find` | `search` |
| `index-search` | `search` |
| `cp` | `copy` |
| `rm` | `delete` |
| `map` | `atlas` |
| `sync` | `refresh` |
| `fix` | `repair` |
| `where` | `detect` |
| `self-install` | `install` |
| `self-update` | `upgrade` |
| `mv` | `move` |

## 自检

`abook selftest` 跑内置自检（默认 100+ 项，`--verbose` 逐条列出，`--only N` 只跑第 N 条解析用例便于调试）。它覆盖解析、校验、
正则、模糊、搜索分级、JSON 输出、分页恒等、写类命令默认预演等；有失败项时退出码 5。

CI 与本地都用它当唯一验收闸门：改完代码先跑 `selftest`，再跑
`python3 tools/check_docs.py`、`python3 tools/check_workflow.py`、`python3 tools/check_nowrite.py`。

## 维护与平台

| 命令 | 作用 |
|---|---|
| `init` | 建库结构（含地图、名册、保险箱），已存在则跳过 |
| `selftest [--verbose]` | 内置自检，覆盖解析、校验、正则、模糊、搜索分级、JSON 输出等 |
| `detect [--json]` | 平台识别 + 库根自检（程序是否与库同目录、库是否已初始化） |
| `install [--url URL]` | 首次安装：下载本机架构的二进制，放进库根 |
| `upgrade [--check] [--force] [--url URL]` | 已装则换新版：先取 manifest 再决定是否下载，替换 `current_exe()` 并过 sha256、自报版本、`selftest` 三道；`--check` 只查不装 |
| `version` | 版本 |
| `help` | 帮助 |

`install` 会用 GitHub Releases API 取本机架构的资产；私有仓库需要 `ABOOK_TOKEN`。

`upgrade` 与 `install` 的区别：前者是"已经装好了，换新版"，会先取 manifest 再决定要不要下，
替换自身并过 sha256、自报版本、`selftest` 三道；后者是"这台机器还没有"。

**Android 上不要用 `current_exe()` 判断自身位置。** 内核 exec 的是
`/apex/com.android.runtime/bin/linker64`，由链接器加载程序，所以 `/proc/self/exe` 指向链接器
而不是 abook。照它算安装目标等于准备往只读系统分区覆盖链接器。`util::self_path()` 先读
`/proc/self/cmdline` 拿真实路径，并在兜底时用 `is_linker()` 显式拒绝链接器名字；拿不到就返回
None，让调用方用 `--dir` 显式指定，而不是替它猜。已有自检锁住这条。

## 退出码

| 码 | 含义 |
|---|---|
| 0 | 成功 |
| 2 | 用法错误（缺参数、未知命令、未知参数名） |
| 3 | 未找到（记录、凭据、召回无命中） |
| 4 | 需批准或被阻塞（正式库改动未加 `--approve`） |
| 5 | 完整性问题（对账不符、写入复读失败、自检有失败项） |
| 6 | IO 错误 |
