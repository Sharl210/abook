# abook 安装与使用

单二进制、零依赖。记忆库的全部操作都在一个 `abook` 命令里。

## 一、先判断要不要装

```sh
abook detect
```

三种结果：

| 输出 | 含义 | 下一步 |
|---|---|---|
| `结论 就绪` | 程序就在库根，且库已初始化 | 直接用 |
| `结论 程序已在库根，但库还没建起来` | 位置对，库是空的 | `abook init` |
| `结论 程序不在库根` | 二进制在别处 | `abook install`（装到库根） |
| 命令本身找不到 | 完全没装 | 用下面的手工方式装 |

`abook detect --json` 给机器读的版本，字段：`os` `arch` `target` `exe` `self` `root` `initialized` `in_root` `ready`。

注意判定标准变了：以前问的是“PATH 里能不能直接敲 `abook`”，现在问的是
“这个程序是不是和库放在同一个目录”。前者每换一个 shell 就有不同答案，后者
只取决于文件实际在哪。

## 二、装

### 自动

```sh
abook install
```

按 `detect` 识别出的架构下载对应产物，落到**库根目录**（默认 `~/AgentsBook`），
完事打印库根和可执行文件的绝对路径。不写 `PATH`、不改 shell 配置、不碰系统目录。

自定义下载源（默认 GitHub Releases）：

```sh
abook install --url https://你的地址
# 或
export ABOOK_BASE_URL=https://你的地址
```

### 直接跑脚本装

不想先有 abook 时用仓库里的 `install.sh`：

```sh
sh install.sh                       # 自动识别平台
sh install.sh --bin-dir ~/bin       # 指定安装目录
sh install.sh --token <凭据>         # 私有仓库下载用（也可用 ABOOK_TOKEN）
sh install.sh --self-check          # 离线验证架构/平台/产物名映射，不联网
```

私有仓库的 `github.com/.../releases/download` 不认 `Authorization`，只会返回 404；
脚本会自动改走 API 资源接口，所以凭据是必需的。

`--token` 曾经只写在报错提示和本文档里，脚本本身根本没有解析它，于是值被当成
位置参数丢掉、凭据为空、API 兜底整段被跳过，用户只看到一句「下载失败」。
现在 `tools/check_workflow.py` 会机械比对「脚本宣传的参数」与「脚本真的处理的参数」，
对不上就报错，这项检查已进预检。

### 手工

从 Releases 下载对应文件，改名去掉后缀，放进你的记忆库根目录
（默认 `~/AgentsBook`，里面应当已有 `Global.md`/`Index.md`）：

| 平台 | 架构 | 文件 |
|---|---|---|
| Linux | x86_64 | `abook-x86_64-unknown-linux-musl` |
| Linux | aarch64 | `abook-aarch64-unknown-linux-musl` |
| Linux | armv7 | `abook-armv7-unknown-linux-musleabihf` |
| Linux | i686 | `abook-i686-unknown-linux-musl` |
| Linux | riscv64 | `abook-riscv64gc-unknown-linux-gnu` |
| Linux | loongarch64 | `abook-loongarch64-unknown-linux-gnu` |
| Android | aarch64 | `abook-aarch64-linux-android` |
| Android | armv7 | `abook-armv7-linux-androideabi` |
| Android | x86_64 | `abook-x86_64-linux-android` |
| Android | i686 | `abook-i686-linux-android` |
| Windows | x86_64 | `abook-x86_64-pc-windows-gnullvm.exe` |
| Windows | aarch64 | `abook-aarch64-pc-windows-gnullvm.exe` |
| Windows | i686 | `abook-i686-pc-windows-gnullvm.exe` |
| macOS | x86_64 | `abook-x86_64-apple-darwin` |
| macOS | aarch64 | `abook-aarch64-apple-darwin` |

Linux 用 musl 静态链接，不依赖系统 glibc 版本，任何发行版都能跑。

## 三、库根怎么定

**程序自己所在的目录就是库根。** `abook` 和 `Global.md` / `Index.md` 放在同一层，
所以它从哪个 shell 被调用、有没有 `export` 过什么，答案都一样。

解析顺序：

1. `--root <路径>` 显式指定
2. 程序所在目录（该目录含 `Global.md`、`Index.md`、`canonical/` 或 `candidate/` 时）
3. `ABOOK_ROOT`（仅为老脚本兼容保留，可留空）
4. `~/AgentsBook`

把库挪到别处就两条路：连带二进制一起搬过去，或者每次带 `--root`。**不要**用
`ABOOK_ROOT` 指一个和程序不同的库——那等于让“我在哪”和“我的数据在哪”给出两个
答案，正是这次改造要去掉的东西。

## 四、首次接管一个已有的库

不需要转换，直接读：

```sh
abook doctor          # 健康检查
abook state           # 记录数 / 地图数 / 收录时刻 / 一致性
abook reconcile       # 目录 vs 地图 vs 名册三方对账
```

## 五、预编译包是怎么出来的

发布页上每个平台一个文件，由 GitHub Actions 从同一份源码交叉编译。
想看某个平台为什么没出来，先看它的作业是不是在 `Install cross toolchains`
这一步就挂了——这一步的失败最容易被误读。

### 一个已经踩过的坑

`aarch64-unknown-linux-musl` 曾经连续失败三次，每次报的都是
`The action 'Install cross toolchains' has timed out after 15 minutes`。
看到这句会以为是网络慢，其实和网络无关：

1. 要装的那批 Debian 包里写了 `libc6-dev-aarch64-cross`。
2. Debian 的目标架构命名和三元组不是一套：三元组叫 `aarch64`，Debian 包名叫
   **`arm64`**。所以这个名字在仓库里根本不存在。
3. `apt-get download a b c ...` 只要有一个名字不认识，**整批一个都不下**，
   于是解包出 0 个文件，也没有任何编译器的影子。
4. 代码发现没有编译器就回退到 `apt-get install`，那里等 1800 秒；
   而这一步的总预算只有 900 秒。
5. 于是真正的错因（包名不存在）被“超时”盖住了。

对应到现在的做法：

- 包名逐个下载，哪个名字不认识就直接点名报错，不再整批连坐。
- 回退等待压到 300 秒，保证失败发生在预算内，能看到原因。
- 本地 `tools/check_workflow.py` 会校验每个 `*-cross` 包名用的是不是
  Debian 架构名（`aarch64→arm64`、`armv7→armhf`、`x86_64→amd64`），
  写错在本地就红，不用等 CI。
- 同一脚本还会把 workflow 里每个 shell 块丢给 `bash -n` 跑一遍，
  语法错误同样在本地拦下。

### 发布页和矩阵对不对得上

发布页只有 15 个资产，矩阵也只有 15 个目标，但两边是各走各的路：
某次构建挂了，那个平台就永远停在一个月前的版本；某个目标从矩阵里删了，
它的旧资产还挂在发布页上被 `install` 下载。这两种都不会在本地暴露。

把发布页当前的文件名交给同一个脚本，它和矩阵对一遍：

```sh
python3 tools/check_workflow.py --release-audit $(curl -s \
  -H "Authorization: Bearer $TOKEN" \
  https://api.github.com/repos/<owner>/<repo>/releases/tags/latest \
  | python3 -c 'import json,sys;print(" ".join(a["name"] for a in json.load(sys.stdin)["assets"]))')
```

缺哪个、多哪个都直接点名，退出码非零。Windows 的 `.exe` 由目标里的 OS 段
决定，不看 `-gnu` 后缀——Linux 目标也带 `-gnu`，靠后缀判断会冤枉一半。

### 收窄重试不会发布

只想重试一个平台时给 `only` 传那个三元组：

```sh
curl -X POST -H "Authorization: Bearer $TOKEN" \
  -H "Accept: application/vnd.github+json" \
  https://api.github.com/repos/<owner>/<repo>/actions/workflows/build.yml/dispatches \
  -d '{"ref":"main","inputs":{"only":"aarch64-unknown-linux-musl"}}'
```

`only` 非空时构建作业只跑该目标，发布作业整体跳过——否则单个产物会把
整个 release 覆盖掉。`aarch64-unknown-linux-musl` 这一类名字可以和别的
目标用逗号并列，一次重试多个。
