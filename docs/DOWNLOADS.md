# 固定下载地址

提示词里只需要写**一条不含版本号**的地址。发版时这里不动，地址始终指向最新构建。

## 两种目录形态

| 用途 | 地址 | 形态 |
|---|---|---|
| 官方 release | `https://github.com/Sharl210/abook/releases/latest/download/<资产名>` | 平铺 `abook-<triple>[.exe]` |
| 固定分支 | `https://raw.githubusercontent.com/Sharl210/abook/downloads/<triple>/abook[.exe]` | 每目标一目录 |
| 清单 | `https://raw.githubusercontent.com/Sharl210/abook/downloads/manifest.json` | JSON |
| 校验和 | `https://raw.githubusercontent.com/Sharl210/abook/downloads/SHA256SUMS` | `sha256sum -c` 格式 |

`downloads` 分支每次构建整体覆盖（单孤儿提交 force push），不做增量，所以仓库不会随发版
无限长胖。它的内容由 `.github/workflows/build.yml` 的 `downloads` 作业生成，少于 15 个
二进制时该作业直接失败、不覆盖分支——宁可这次不更新，也不让固定地址出现残缺。

## 镜像

国内直连 github.com 常年不稳。下列前缀把官方整条地址原样拼在后面即可：

```
https://gh-proxy.com/<官方地址>
https://ghfast.top/<官方地址>
https://ghproxy.net/<官方地址>
```

另有一个不同线路的源，只对 `downloads` 分支有效（它只代理仓库文件，不代理 release 资产）：

```
https://cdn.jsdelivr.net/gh/Sharl210/abook@downloads/<triple>/abook
```

例：

```
https://gh-proxy.com/https://raw.githubusercontent.com/Sharl210/abook/downloads/x86_64-unknown-linux-musl/abook
https://cdn.jsdelivr.net/gh/Sharl210/abook@downloads/x86_64-unknown-linux-musl/abook
```

**这里不排优先级。** 镜像的可用性和速度高度依赖你所在网络，无法远程测准。`install.sh`
的做法是逐个试、谁先成功用谁，每个源都要通过产物格式校验（前 4 字节是 ELF/PE/Mach-O），
HTTP 200 不算成功——劫持页和错误页也是 200。

## 15 个目标

| triple | os | arch | libc | 文件名 |
|---|---|---|---|---|
| `aarch64-linux-android` | android | aarch64 | bionic | `abook` |
| `armv7-linux-androideabi` | android | armv7 | bionic | `abook` |
| `i686-linux-android` | android | i686 | bionic | `abook` |
| `x86_64-linux-android` | android | x86_64 | bionic | `abook` |
| `aarch64-unknown-linux-musl` | linux | aarch64 | musl | `abook` |
| `armv7-unknown-linux-musleabihf` | linux | armv7 | musl | `abook` |
| `i686-unknown-linux-musl` | linux | i686 | musl | `abook` |
| `x86_64-unknown-linux-musl` | linux | x86_64 | musl | `abook` |
| `loongarch64-unknown-linux-gnu` | linux | loongarch64 | glibc | `abook` |
| `riscv64gc-unknown-linux-gnu` | linux | riscv64 | glibc | `abook` |
| `aarch64-pc-windows-gnullvm` | windows | aarch64 | mingw-llvm | `abook.exe` |
| `i686-pc-windows-gnullvm` | windows | i686 | mingw-llvm | `abook.exe` |
| `x86_64-pc-windows-gnullvm` | windows | x86_64 | mingw-llvm | `abook.exe` |
| `aarch64-apple-darwin` | macos | aarch64 | libSystem | `abook` |
| `x86_64-apple-darwin` | macos | x86_64 | libSystem | `abook` |

四个 musl 目标全静态、无解释器依赖，任何 Linux 上直接跑。两个 glibc 目标需要目标机 glibc
不低于构建机（Ubuntu latest）。

## 怎么选

按 `uname -s` 与 `uname -m` 取，Android 还要看 `getprop ro.build.version.sdk` 与 ABI。
`install.sh --self-check` 里那套映射就是权威实现，可以直接读它，不要另写一套：

```
sh install.sh --self-check
```

`manifest.json` 里每条都带 `os`/`arch`/`libc`/`size`/`sha256`，按字段过滤即可，不必解析
文件名。

## 最小调用

```sh
# 官方直连（海外或已代理）
curl -fsSL https://raw.githubusercontent.com/Sharl210/abook/main/install.sh | sh

# 走镜像装脚本本身
curl -fsSL https://gh-proxy.com/https://raw.githubusercontent.com/Sharl210/abook/main/install.sh | sh
```

`install.sh` 会自动按 `uname` 选目标、走上面 9 个源依次尝试、校验产物格式、放进 `PATH`。
