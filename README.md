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

```
~/AgentsBook/
  Global.md               入口，带上记录数与索引摘要
  Index.md                路由表：什么话题去看哪一条
  canonical/              正式库：已验收的知识
    Atlas.md              地图：每条记录归在哪个分支
    records/<ID>.md       一条记录
    records/<ID>-refs/    该记录的附件（截图、日志、原始数据）
  candidate/              候选库：待验证或待批准的内容
  staging/                待决件
  review/                 晋升记录
  commits/                变更日志
  vault/                  凭据保险箱（加密）
```

一条记录是带标签的文本：

```
FORMAT=CANONICAL_MEMORY_V2
ID=AB2-IMS-051
KIND=EVIDENCE
TITLE=IMS 注册失败在三个平台上的差异
KEYS=IMS,注册,REGISTER
READ_WHEN=遇到 IMS 注册相关问题时
[SECTIONS]
[MEMORY_BODY]
正文…
[/MEMORY_BODY]
```

`KEYS` 和 `READ_WHEN` 是召回索引的来源，不是装饰——写记录时漏掉它们，这条内容
以后就只能靠全文搜索找到。

## 常用命令

```sh
abook state                      # 库状态一屏：记录数、地图数、一致性
abook recall 关键词              # 召回：词面 → 适用条件 → 分支摘要
abook show <记录>                # 全字段加正文
abook search 词                  # 全文检索，五级精度
abook write --title T --read-when W --body-file F
abook set <记录> TITLE 新标题
abook promote <候选> --approve
abook vault put 标识 --purpose 用途
```

完整清单 `abook commands`，单条契约 `abook describe <命令>`。

## 两条使用纪律

**一、写正式库要明确批准。** `promote`、对 canonical 记录的修改，不带 `--approve`
会被拒绝。这是设计，不是故障——正式库是已经验证过的结论，静默改写它的代价比多
一次确认大。

**二、改动前先读版本号。** `show` 和 `cat` 会在末尾打印 `[version xxxxxxxx]`。
把它带进写命令的 `--if-version`：

```sh
abook show C-20261022T113737-3014f5dc        # 看到 [version 3f2a1b8c]
abook set C-20261022T113737-3014f5dc TITLE 新标题 --if-version 3f2a1b8c
```

如果在这两次命令之间别的东西改过这条记录，`set` 会拒绝并告诉你双方版本。不带
`--if-version` 也能写，行为与以前一样——但那意味着你放弃了这层保护。

## 文档

- `docs/MAINTENANCE.md` — 维护者须知：架构、数据格式的坑、缺陷史、发版流程
- `docs/DOWNLOADS.md` — 固定下载地址与镜像

## 许可

内部工具。
