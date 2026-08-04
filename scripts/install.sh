#!/bin/sh
# monitor-agent 一键安装脚本
#
# 用法：
#   curl -fsSL <发布地址>/install.sh | sh -s -- --key <KEY> --label <NAME>
#   可选参数：
#     --url <URL>       接收端地址（默认用包内配置）
#     --version <ver>   版本号或 latest（默认 latest）
#   先下载再用：
#     sh install.sh --key xxx --label web-01
#
# 环境变量：
#   MONITOR_AGENT_REPO  发布下载基址（默认见下方，请改为你的发布地址）

set -e

REPO_BASE="${MONITOR_AGENT_REPO:-https://github.com/bevin1984/monitor-agent/releases}"
KEY=""
LABEL=""
SERVER_URL=""
VERSION="latest"

while [ $# -gt 0 ]; do
  case "$1" in
    --key) KEY="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    --url) SERVER_URL="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    -h|--help) sed -n '2,16p' "$0" 2>/dev/null; exit 0 ;;
    *) echo "未知参数: $1（--help 查看用法）" >&2; exit 1 ;;
  esac
done

if [ -z "$KEY" ]; then
  echo "错误：必须提供 --key <KEY>" >&2
  exit 1
fi

# 识别架构
ARCH=$(uname -m)
case "$ARCH" in
  x86_64|amd64)  RUST_ARCH="x86_64";  DEB_ARCH="amd64" ;;
  aarch64|arm64) RUST_ARCH="aarch64"; DEB_ARCH="arm64" ;;
  *) echo "错误：不支持的架构 $ARCH" >&2; exit 1 ;;
esac

# 识别发行版/包类型
PKG_TYPE=""
if [ -f /etc/debian_version ] || grep -qiE '^(ID|ID_LIKE)=.*(debian|ubuntu)' /etc/os-release 2>/dev/null; then
  PKG_TYPE="deb"
elif [ -f /etc/redhat-release ] || grep -qiE '^(ID|ID_LIKE)=.*(rhel|centos|rocky|almalinux|anolis|fedora)' /etc/os-release 2>/dev/null; then
  PKG_TYPE="rpm"
fi
if [ -z "$PKG_TYPE" ]; then
  echo "错误：无法识别发行版（仅支持 rpm/deb 系）" >&2
  exit 1
fi

# 下载地址
if [ "$VERSION" = "latest" ]; then
  DL="${REPO_BASE}/latest/download"
else
  DL="${REPO_BASE}/download/${VERSION}"
fi
if [ "$PKG_TYPE" = "deb" ]; then
  FILE="monitor-agent-${DEB_ARCH}.deb"
else
  FILE="monitor-agent-${RUST_ARCH}.rpm"
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
PKG="${TMP}/${FILE}"

echo "==> 下载 ${DL}/${FILE}"
if command -v curl >/dev/null 2>&1; then
  curl -fsSL "${DL}/${FILE}" -o "$PKG"
elif command -v wget >/dev/null 2>&1; then
  wget -qO "$PKG" "${DL}/${FILE}"
else
  echo "错误：需要 curl 或 wget" >&2; exit 1
fi

echo "==> 安装包（$PKG_TYPE）"
if [ "$PKG_TYPE" = "deb" ]; then
  if command -v apt-get >/dev/null 2>&1; then
    apt-get install -y "$PKG" >/dev/null
  else
    dpkg -i "$PKG" || apt-get install -fy >/dev/null
  fi
else
  if command -v dnf >/dev/null 2>&1; then
    dnf install -y "$PKG" >/dev/null
  elif command -v yum >/dev/null 2>&1; then
    yum install -y "$PKG" >/dev/null
  else
    rpm -Uvh --force "$PKG"
  fi
fi

# 写入配置
CFG=/etc/monitor-agent/config.toml
echo "==> 写入配置 $CFG"
set_conf() {
  if grep -q "^$1 *=" "$CFG" 2>/dev/null; then
    sed -i "s|^$1 *=.*|$1 = \"$2\"|" "$CFG"
  else
    echo "$1 = \"$2\"" >> "$CFG"
  fi
}
set_conf key "$KEY"
[ -n "$LABEL" ] && set_conf label "$LABEL"
[ -n "$SERVER_URL" ] && set_conf url "$SERVER_URL"
chown -R monitor-agent:monitor-agent /etc/monitor-agent 2>/dev/null || true
chmod 640 "$CFG" 2>/dev/null || true

echo "==> 启用并启动服务"
systemctl daemon-reload
systemctl enable monitor-agent
systemctl restart monitor-agent

echo "==> 完成。状态: systemctl status monitor-agent  日志: journalctl -u monitor-agent -f"
