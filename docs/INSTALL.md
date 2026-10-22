# abook 安装与使用

单二进制、零依赖。记忆库的全部操作都在一个 `abook` 命令里。

## 一、先判断要不要装

```sh
abook detect
```

三种结果：

| 输出 | 含义 | 下一步 |
|---|---|---|
| `结论 就绪` | PATH 里已有裸名 `abook` | 直接用 |
| `结论 需要初始化` | 没有，或 PATH 里找不到 | `abook install` |
| 命令本身找不到 | 完全没装 | 用下面的手工方式装 |

`abook detect --json` 给机器读的版本，字段：`os` `arch` `target` `exe` `on_path` `ready` `found` `self` `root`。

## 二、装

### 自动

```sh
abook install
```

按 `detect` 识别出的架构下载对应产物，落到 `~/.local/bin/abook`，然后提示你把该目录加进 PATH。

自定义下载源（默认 GitHub Releases）：

```sh
abook install --url https://你的地址
# 或
export ABOOK_BASE_URL=https://你的地址
```

### 手工

从 Releases 下载对应文件，改名去掉后缀放进 PATH 里的任一目录：

| 平台 | 架构 | 文件 |
|---|---|---|
| Linux | x86_64 | `abook-x86_64-unknown-linux-musl` |
| Linux | aarch64 | `abook-aarch64-unknown-linux-musl` |
| Linux | armv7 | `abook-armv7-unknown-linux-musleabihf` |
| Linux | i686 | `abook-i686-unknown-linux-musl` |
| Linux | riscv64 | `abook-riscv64gc-unknown-linux-musl` |
| Linux | loongarch64 | `abook-loongarch64-unknown-linux-musl` |
| Android | aarch64 | `abook-aarch64-linux-android` |
| Android | armv7 | `abook-armv7-linux-androideabi` |
| Android | x86_64 | `abook-x86_64-linux-android` |
| Android | i686 | `abook-i686-linux-android` |
| Windows | x86_64 | `abook-x86_64-pc-windows-gnu.exe` |
| Windows | aarch64 | `abook-aarch64-pc-windows-gnullvm.exe` |
| Windows | i686 | `abook-i686-pc-windows-gnu.exe` |
| macOS | x86_64 | `abook-x86_64-apple-darwin` |
| macOS | aarch64 | `abook-aarch64-apple-darwin` |

Linux 用 musl 静态链接，不依赖系统 glibc 版本，任何发行版都能跑。

## 三、记忆根

默认 `~/AgentsBook`。改法：

```sh
export ABOOK_ROOT=/path/to/AgentsBook
```

或每条命令带 `--root`。

## 四、首次接管一个已有的库

不需要转换，直接读：

```sh
abook doctor          # 健康检查
abook state           # 记录数 / 地图数 / 收录时刻 / 一致性
abook reconcile       # 目录 vs 地图 vs 名册三方对账
```
