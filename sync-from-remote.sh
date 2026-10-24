#!/bin/sh
# 从远程取回**CI 编译好的**本机架构二进制，替换本地安装。
#
# 为什么方向是这个：CI 一次编译 15 个架构，本机架构只是其中之一。远程那份是在
# 受控环境里统一构建的，也是用户实际下载到的文件。本地手工编的那份即使源码指纹
# 相同，字节也可能不同（工具链版本、构建路径、release profile 细节）。
#
# 把远程产物取回来装上，本地与用户拿到的就是**同一个文件**，不是「大概一致」。
# 反过来（让远程对齐本地）做不到：本机只能编 1 个架构，其余 14 个只能在 CI 里出。
#
# 用法：sh sync-from-remote.sh [架构]     默认取本机架构
#       sh sync-from-remote.sh --check    只比对，不安装
set -u

REPO="Sharl210/abook"
DEST="${DEST:-/data/data/com.dsharnessmobile.shell/files/usr/bin/abook}"

# 本机架构 → 目标三元组
detect_triple() {
  os=$(uname -s); arch=$(uname -m)
  case "$os" in
    Linux)
      if [ -e /system/app ] || [ -n "${ANDROID_ROOT:-}" ]; then
        case "$arch" in
          aarch64|arm64) echo aarch64-linux-android ;;
          armv7l|armv8l) echo armv7-linux-androideabi ;;
          i686)          echo i686-linux-android ;;
          x86_64)        echo x86_64-linux-android ;;
        esac
      else
        case "$arch" in
          x86_64|amd64) echo x86_64-unknown-linux-musl ;;
          aarch64|arm64) echo aarch64-unknown-linux-musl ;;
        esac
      fi ;;
    Darwin)
      case "$arch" in
        arm64|aarch64) echo aarch64-apple-darwin ;;
        *)             echo x86_64-apple-darwin ;;
      esac ;;
  esac
}

TRIPLE="${1:-}"
case "$TRIPLE" in
  --check|check) MODE=check;  TRIPLE=$(detect_triple) ;;
  "")            MODE=install; TRIPLE=$(detect_triple) ;;
  -*)            echo "未知选项: $TRIPLE（可用: --check）" >&2; exit 2 ;;
  *)             MODE=install ;;
esac
[ -n "$TRIPLE" ] || { echo "无法判定本机架构，请显式给 triple" >&2; exit 2; }

# 校验 triple 是否在 CI 的发布清单里。
#
# 不校验的话，任何写错的字符串都会被当成架构名拿去拼下载地址，然后四个镜像全部
# 404，最后只打印一句「所有源都失败」——真正的错因（架构名写错了）被埋在下面。
# 失败要指向原因，不能只说结果。
case "$TRIPLE" in
  aarch64-linux-android|armv7-linux-androideabi|i686-linux-android|x86_64-linux-android|  x86_64-unknown-linux-musl|aarch64-unknown-linux-musl|armv7-unknown-linux-musleabihf|  i686-unknown-linux-musl|loongarch64-unknown-linux-gnu|riscv64gc-unknown-linux-gnu|  aarch64-apple-darwin|x86_64-apple-darwin|  aarch64-pc-windows-gnullvm|x86_64-pc-windows-gnullvm|i686-pc-windows-gnullvm) ;;
  *) echo "未知架构: $TRIPLE" >&2
     echo "可用架构共 15 个，例如 aarch64-linux-android / x86_64-unknown-linux-musl。" >&2
     echo "只想比对不安装用 --check。" >&2
     exit 2 ;;
esac

case "$TRIPLE" in *windows*) SUF=.exe ;; *) SUF= ;; esac

WORK="target/remote-$TRIPLE"
mkdir -p "$WORK"

# 源顺序与提示词里的冷启动**不同**，这里是故意的：
#
# 那个顺序为国内被墙的用户设计，镜像优先。而这个脚本是「我编完要跟远程对账」，
# 正确性优先于可达性——jsDelivr 会对 @downloads 分支做长时间缓存，实测取回的是
# 上一版的旧文件，指纹对不上。raw.githubusercontent.com 每次 force push 后立即可读，
# 所以它排第一；镜像次之；jsDelivr 放最后，只在前面的都不通时才用。
fetch() {
  for url in \
    "https://raw.githubusercontent.com/$REPO/downloads/$TRIPLE/abook$SUF" \
    "https://gh-proxy.com/https://raw.githubusercontent.com/$REPO/downloads/$TRIPLE/abook$SUF" \
    "https://ghfast.top/https://raw.githubusercontent.com/$REPO/downloads/$TRIPLE/abook$SUF" \
    "https://ghproxy.net/https://raw.githubusercontent.com/$REPO/downloads/$TRIPLE/abook$SUF" \
    "https://github.com/$REPO/releases/latest/download/abook-$TRIPLE$SUF" \
    "https://cdn.jsdelivr.net/gh/$REPO@downloads/$TRIPLE/abook$SUF"
  do
    [ -n "${VERBOSE:-}" ] && echo "  试 $url" >&2
    if curl -fsSL --connect-timeout 8 -m 120 -o "$WORK/abook$SUF" "$url" 2>/dev/null; then
      # HTTP 200 不算成功：劫持页、登录页也都是 200。只认文件头。
      head4=$(head -c 4 "$WORK/abook$SUF" 2>/dev/null | od -An -tx1 | tr -d ' \n')
      case "$head4" in
        7f454c46|4d5a*|cafebabe|feedface|feedfacf|cefaedfe|cffaedfe)
          echo "取回成功: $url" >&2; return 0 ;;
        *) echo "  不是可执行文件，换下一个源" >&2 ;;
      esac
    fi
  done
  return 1
}

echo "架构: $TRIPLE"
echo "目标: $DEST"
echo
fetch || { echo "所有源都失败" >&2; exit 1; }

chmod +x "$WORK/abook$SUF"

# 远程产物的源码指纹必须能取出来，否则它是不是这份源码编的就无从判断
REMOTE_FP=$(LC_ALL=C grep -aoE 'src=[0-9a-f]{16}' "$WORK/abook$SUF" 2>/dev/null | head -1 | sed 's/src=//')
if [ -z "$REMOTE_FP" ]; then
  # 本机架构的产物可以直接问它自己
  REMOTE_FP=$("$WORK/abook$SUF" version 2>/dev/null | sed -n 's/.*src=\([0-9a-f]\{16\}\).*/\1/p')
fi
echo "远程指纹: ${REMOTE_FP:-读不出}"

# 工作树的源码指纹：直接问当前源码编出来的那份。
#
# 这里**不能**读 $DEST 的指纹：那是"磁盘上现在装着什么"，本次运行之后它就会被
# 换掉，与被换上的那份比是没有意义的（一个是被替换文件的历史值，一个是远程值）。
# 真正该回答的问题是"远程这份和我的源码是不是同源"，所以要问的是源码自己——
# 让 cargo 静默编一次，问它报什么。
WORKTREE_FP=""
if command -v cargo >/dev/null 2>&1; then
  cargo build --release --quiet >/dev/null 2>&1
  if [ -x "target/release/abook" ]; then
    WORKTREE_FP=$(./target/release/abook version 2>/dev/null | sed -n 's/.*src=\([0-9a-f]\{16\}\).*/\1/p')
  fi
fi
echo "工作树指纹: ${WORKTREE_FP:-无法计算（缺 cargo，跳过同源核对）}"

if [ -n "$WORKTREE_FP" ] && [ "$WORKTREE_FP" != "$REMOTE_FP" ]; then
  echo
  echo "远程产物与工作树源码不同源：" >&2
  echo "  远程: $REMOTE_FP" >&2
  echo "  工作树: $WORKTREE_FP" >&2
  echo "说明你要装的这份不是由当前源码编出来的。可能原因：tag 推早了、CI 缓存、" >&2
  echo "或本地还有未提交的改动。先确认这两者为何不同，不要直接装上。" >&2
  exit 1
fi

if [ "$MODE" = check ]; then
  LOCAL_FP=""
  [ -x "$DEST" ] && LOCAL_FP=$("$DEST" version 2>/dev/null | sed -n 's/.*src=\([0-9a-f]\{16\}\).*/\1/p')
  if [ "$REMOTE_FP" = "$LOCAL_FP" ] && [ "$REMOTE_FP" = "$WORKTREE_FP" ]; then
    echo "结论: 已一致，无需更新"
  else
    echo "结论: 需要同步（已装:${LOCAL_FP:-无} 远程:$REMOTE_FP 工作树:${WORKTREE_FP:-?}）"
    exit 1
  fi
  exit 0
fi

# 装之前先确认远程这份能跑：架构不符或文件损坏在这里暴露，
# 而不是装上去之后连 `abook state` 都跑不了。
if ! "$WORK/abook$SUF" version >/dev/null 2>&1; then
  echo "远程产物在本机跑不起来（架构不符或已损坏），拒绝安装" >&2
  "$WORK/abook$SUF" version 2>&1 | head -3 >&2
  exit 1
fi

# 原子替换：临时文件放目标同目录再 mv，跨文件系统 rename 会失败
TMPD=$(dirname "$DEST")
cp "$WORK/abook$SUF" "$TMPD/.abook.new"
chmod +x "$TMPD/.abook.new"
mv -f "$TMPD/.abook.new" "$DEST"

echo
echo "已安装。核对："
"$DEST" version | head -1
echo "sha256: $(sha256sum "$DEST" | cut -d' ' -f1)"
"$DEST" selftest 2>&1 | tail -1
