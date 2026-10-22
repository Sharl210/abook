# abook 命令参考

所有命令都接受 `--root <路径>` 与 `--json`（有输出的命令）。

## 读

| 命令 | 作用 |
|---|---|
| `state` | 记录数、地图数、收录时刻、最新变更、一致性结论 |
| `stats` | KIND / MATURITY / 分支 / 已晋升分布 |
| `recall <词>` | 三级召回：词面 → READ_WHEN → 片段回退 → 分支摘要 |
| `search <模式>` | 全文与文件名搜索，支持 `*` `?` |
| `show <记录>` | 看一条记录的完整内容 |
| `atlas [lib]` | 分支地图；`--verbose` 显示声明数与实析数 |
| `index [词]` | Index.md 路由表 |
| `recent` | 最近变更记录 |
| `paths` | 打印所有路径 |
| `lint` | 头部完整性与命名规范 |
| `doctor [--fix]` | 健康检查 |
| `reconcile` | 目录 / 地图 / 名册三方对账 |

### 记录选择器

`show` `update` `delete` `restore` `promote` 都接受四种写法：

- 完整路径
- 库内相对路径 `canonical/records/C-xxx.md`
- 文件名 `C-xxx.md` 或 `C-xxx`
- 记录 ID 或**标题**（标题前缀唯一即可）

多条匹配会报错并列出候选，不会瞎猜。

### 召回分级

```
词面      KEYS / CONCEPTS / ENTITIES / TITLE / ID 命中    100~60 分
READ_WHEN READ_WHEN / SUMMARY 命中                        40~20 分
片段回退  中文按 2-gram 切窗，要求过半窗口命中            按覆盖率计
分支摘要  词面全落空时才启用，按分支 KEYS/SUMMARY 兜底
```

`--include-promoted` 把已晋升的候选也纳入（默认不纳入）。
`--body` 连正文一起输出。

## 写

正式库（`--lib canonical`）的任何变更都要 `--approve`，没有就返回退出码 4 并说明原因。

| 命令 | 说明 |
|---|---|
| `write` | 新建记录 |
| `promote <候选>` | 候选 → 正式，并在候选写 `[PROMOTION]` 标记 |
| `update <记录>` | 改指定字段 |
| `delete <记录>` | 默认软删除（`STATUS=DELETED`），`--hard` 才物理删 |
| `restore <记录>` | 软删除恢复 |
| `commit` | 追加一条变更记录 |

### write 的字段

必填：`--title` `--read-when`，正文用 `--body` 或 `--body-file`。

常用可选：`--kind` `--maturity` `--authority` `--summary` `--keys` `--concepts`
`--entities` `--not-for` `--evidence` `--uncertainty` `--source` `--related`
`--supersedes` `--branch` `--project` `--task` `--session`。

`--dry-run` 只打印将写入的内容。

### 一次写入会同步更新

1. `records/` 下的记录文件（临时文件 + 复读校验 + 原子改名）
2. 该库的 `Atlas.md`（分支 PATH / COUNT / MAXSIZE / REVISION / 收录时刻）
3. `Index.md` 的路由行
4. `Global.md` 的记录数与两图收录时刻
5. `commits/` 下一条变更记录

任一步失败都保留最近有效版本并返回非零退出码。

## 保险箱

| 命令 | 说明 |
|---|---|
| `vault list` | 列条目（只解密首行，取标识与用途） |
| `vault get <标识>` | 取明文；`--purpose-only` 只取用途 |
| `vault put <标识> --purpose P` | 写入；正文用 `--body` / `--body-file` / `--stdin` |
| `vault rm <标识>` | 删除，需 `--approve` |
| `vault ledger [--rebuild]` | 看或重建名册 |
| `vault verify` | 逐条解密自检 + 文件名派生校验 |
| `vault names` | 标识 → 派生文件名 |
| `vault reconcile` | 文件数与名册行数对账 |
| `vault export --out F` | 导出全部明文 |

写入后自动解密回读比对，不一致直接判失败，不会把坏条目当成功。

## 维护

| 命令 | 说明 |
|---|---|
| `init` | 建库结构（含名册），已存在则跳过 |
| `refresh [lib]` | 按磁盘重建地图路径并推进收录时刻 |
| `normalize [lib] [--dry-run]` | 旧顺序命名升级为 `前缀-时间戳-哈希8` |
| `detect [--json]` | 平台识别 + PATH 自检 |
| `install [--url URL]` | 下载对应架构二进制 |
| `version` | 版本 |
| `help` | 帮助 |

## 退出码

| 码 | 含义 |
|---|---|
| 0 | 成功 |
| 2 | 用法错误（缺参数、未知命令） |
| 3 | 未找到（记录、凭据、召回无命中） |
| 4 | 需批准或被阻塞 |
| 5 | 完整性问题（对账不符、写入复读失败） |
| 6 | IO 错误 |
