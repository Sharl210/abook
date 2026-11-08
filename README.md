# abook

记忆库（AgentsBook）的命令行工具。一个静态二进制，没有运行时依赖。

记忆不是聊天记录，是可以跨会话复用的知识：某次排障的根因、某个平台版本的差异、
某个坑的绕法。这些内容需要有地方放、能被找回来、能被信任。abook 负责这件事。

## 为什么需要一个工具

最初这些内容就是一堆 Markdown 文件，靠 AI 自己读写。结果反复出现同一类问题：

- 改了正文忘记改头部，之后按头部召回就找不到
- 两个会话同时写，一个成功一个被覆盖，两边都以为写进去了
- 记了 500 条，但地图（Atlas）里的索引和实际文件数对不上，越用越不敢信

这些都是机械工作，交给工具做比交给模型自觉更可靠。所以边界是：**工具管机制，
模型管内容**。命名、原子写入、索引与地图同步、引用完整性、并发互斥都在工具里，
模型只需要知道命令怎么用。

## 安装

```sh
curl -sSfL https://raw.githubusercontent.com/Sharl210/abook/downloads/install.sh | sh
```

或者从 release 下载对应架构的二进制，**放进你的记忆库根目录**（默认
`~/AgentsBook`，和 `Global.md` 放一起）。程序的库根就是它自己所处的目录，不靠
环境变量、也不用改 `PATH`。目标三元组、镜像地址和国内网络的处理见
`docs/DOWNLOADS.md`。

装好后自检：

```sh
~/AgentsBook/abook selftest   # 内置用例，全部在临时目录跑，不碰真实库
~/AgentsBook/abook state      # 库的当前状态
```

用绝对路径调用就行。想少打字就自己做个别名，但不要再把库根目录塞进 `PATH`——
那样会出现两个同名 abook 抢着回答“库在哪”。

## 库长什么样

新库布局：

```text
~/AgentsBook/
  abook.memory.layout.v1
  planes/soul/records/
  planes/rules/records/
  planes/profile/records/
  planes/explicit/records/
  planes/long_term/records/
  planes/episodic/records/
  vault/                  凭据独立存储域
  projections/            普通记忆投影
```

旧 `canonical/`、`candidate/`、`review/`、`commits/` 目录只在历史库兼容与 staged migration 中保留，不是新记忆模型。新记录是结构化 JSON，正文作为完整字符串保存；凭据不进入普通平面索引和 export。


`KEYS` 和 `READ_WHEN` 是召回索引的来源，不是装饰——写记录时漏掉它们，这条内容
以后就只能靠全文搜索找到。

## 常用入口

```sh
abook mcp                         # MCP stdio（现代客户端）
abook memory context --root ROOT  # 读取六个普通平面与凭据域边界
abook memory write --root ROOT --plane explicit --title T --content C
abook memory search --root ROOT --query 关键词
abook memory export --root ROOT --plane all
abook vault put 标识 --purpose 用途 --body-file F
abook upgrade --check --json
```

MCP 客户端优先通过 `server/discover`、`tools/list` 和 `memory_help` 发现完整接口；正文使用结构化参数传输，不拼接 shell 命令。普通记忆平面为 `soul`、`rules`、`profile`、`explicit`、`long_term`、`episodic`，`credentials` 只通过独立 vault 工具访问。

**优先用 MCP 工具，不是命令行。** 凡本服务已把能力暴露成工具，任务与执行就走工具调用；命令行只覆盖工具未涉及的场合以及历史库迁移。同一件事两者都能做时，工具是默认路径。

**环境还没接 MCP 时，先配 MCP 再干活。** 拿到可执行文件后的第一件事是为当前环境接入本服务，配好再开始。配置时把库根写进启动参数一次定下：

```json
{"command": "/path/to/abook", "args": ["mcp", "stdio", "--root", "/path/to/AgentsBook"]}
```

之后所有工具调用都不必再传 `root`。具体怎么配取决于所在环境的实际机制——宿主文档、现有配置文件、自己探查都能给出答案，不必套用别的宿主的写法。

旧 CLI 的 `write`、`promote`、`canonical`、`candidate` 等命令仅用于历史库兼容和迁移，不属于新的 MCP 记忆生命周期。

完整清单 `abook commands`，单条契约 `abook describe <命令>`。

## 写入与并发

新 MCP 记忆直接按平面写入，不需要人工批准；更新、软删除和恢复使用 `expected_revision` 做冲突保护。批量维护使用 `memory_batch_preview`、`memory_batch_apply`、`memory_batch_status` 和 `memory_batch_rollback`。

改动前若使用旧 CLI 记录接口，可用 `show`/`cat` 读取版本号并带入 `--if-version`；这只是兼容入口，不改变新 MCP 平面的结构化接口。

## 文档
- `docs/MAINTENANCE.md` — 维护者须知：架构、数据格式的坑、缺陷史、发版流程
- `docs/DOWNLOADS.md` — 固定下载地址与镜像

## 许可

内部工具。
