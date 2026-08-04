# monitor-agent

轻量级服务器资源监控 agent。周期性采集主机指标，通过 HTTP/HTTPS 把 JSON 推送到接收端。
**只负责「采集 + 推送」**——分析、告警、可视化等业务功能由接收端/看板系统实现。

## 设计目标

- 运行时资源占用最小：CPU <0.1%、常驻内存 <10–15MB、零磁盘写、**不 fork 任何外部命令**
- 同步 `ureq` + `rustls`（无 tokio 异步运行时、无 sysinfo 后台线程、无本地时序存储）
- 一条命令安装、systemd 托管、开机自启
- 兼容主流发行版：CentOS/Anolis/RHEL/Rocky/Alma（RPM）、Debian/Ubuntu（DEB）

## 采集指标

| 类别 | 指标 | 来源 |
|---|---|---|
| CPU | 使用率、1/5/15 分钟负载、运行/总进程数、进程数 | `/proc/stat`、`/proc/loadavg` |
| 内存 | 总量/已用/可用/已用%、swap | `/proc/meminfo` |
| 磁盘空间 | 每个挂载点的总量/已用/可用/已用% | `/proc/mounts` + `statvfs` |
| 磁盘 IO | 每块盘的读写 bytes/s、iops | `/proc/diskstats` |
| 网络 | 每张网卡的 IP（IPv4/IPv6）、进/出累计字节与带宽 bytes/s | `/proc/net/dev` + `getifaddrs` |
| 连接数 | TCP established / time_wait / listen / total | `/proc/net/tcp`(+`tcp6`) |
| 主机 | hostname、machine-id、product_uuid、OS、内核、CPU 型号/核数、uptime | `/proc/*`、`/etc/*` |

## 上报格式

`POST <server_url>`，`Content-Type: application/json`，KEY 通过请求头（默认 `X-Monitor-Key`）携带，body 不含 key。

```json
{
  "schema_version": 1,
  "agent_version": "0.1.2",
  "ts": 1700000000,
  "label": "web-01",
  "host": { "hostname": "...", "machine_id": "...", "os": "Ubuntu 22.04.3 LTS", "cpu_cores": 8, "uptime_secs": 86400 },
  "cpu": { "usage_percent": 12.3, "load_1": 0.1, "running_procs": 2, "total_procs": 461, "process_count": 180 },
  "memory": { "total_bytes": 17179869184, "used_bytes": 8589934592, "used_percent": 50.0 },
  "disks": [ { "device": "/dev/vda1", "mountpoint": "/", "total_bytes": 53687091200, "used_percent": 50.0 } ],
  "disk_io": [ { "device": "vda", "read_bytes_per_sec": 1024.0, "write_iops": 5 } ],
  "networks": [ { "interface": "eth0", "ipv4": ["10.0.1.20"], "ipv6": ["2001:db8:1::a"], "rx_bytes": 1234567890, "rx_bytes_per_sec": 1024.0 } ],
  "tcp": { "established": 42, "time_wait": 130, "listen": 8, "total": 240 }
}
```

单位：容量 bytes、百分比 0–100、`ts` 为 unix 秒（UTC）。差值类（CPU 使用率、带宽、IO 速率）为「上一采集间隔内的平均值」。

> 📊 **接收端/看板对接**：完整字段说明、HTTP 约定、时序与幂等处理、数据量估算、联调方法，见 [看板开发指南.md](看板开发指南.md)。

## 安装

### 一键脚本（推荐）

```sh
curl -fsSL https://raw.githubusercontent.com/bevin1984/monitor-agent/main/scripts/install.sh | sh -s -- --key <你的KEY> --label web-01
```

可选：`--url <接收端URL>`、`--version <ver|latest>`。脚本自动识别发行版与架构。

### 包安装

RPM 系（CentOS/Anolis/RHEL/Rocky/Alma）：
```sh
sudo dnf install ./monitor-agent-0.1.2-1.x86_64.rpm
```
DEB 系（Debian/Ubuntu）：
```sh
sudo apt install ./monitor-agent_0.1.2-1_amd64.deb
```

包安装会自动：创建 `monitor-agent` 系统用户、安装配置到 `/etc/monitor-agent/`、`systemctl enable --now`。

## 配置

配置文件 `/etc/monitor-agent/config.toml`（升级不覆盖本地修改），完整字段见 `/etc/monitor-agent/config.example.toml`。关键项：

```toml
[server]
url        = "https://your-receiver.example.com/data/server"
key        = "你的KEY"          # 也可用环境变量 MONITOR_AGENT_KEY
key_header = "X-Monitor-Key"
ca_bundle_path = ""              # 企业/自签 CA 的 PEM 路径；留空用内置公共 CA
tls_skip_verify = false

[agent]
interval_minutes = 1             # 采集推送频率（分钟）
label = "web-01"                 # 自定义标识；为空时回退 hostname
```

改配置后 `sudo systemctl restart monitor-agent`。

- 环境变量覆盖（优先级高于配置文件）：`MONITOR_AGENT_KEY`、`MONITOR_AGENT_LABEL`、`MONITOR_AGENT_SERVER_URL`、`MONITOR_AGENT_INTERVAL_MINUTES`
- 代理出网：设置 `HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY`（在 unit drop-in 加 `Environment=`）
- 日志级别：`RUST_LOG=info`（默认 warn，成功静默）

## 调试

```sh
monitor-agent --once                                          # 采集一次，JSON 打到 stdout，不推送
monitor-agent --once --config /etc/monitor-agent/config.toml | jq .
monitor-agent --version
journalctl -u monitor-agent -f                                # 日志
systemctl status monitor-agent
```

## 运维注意事项

- **克隆镜像**：批量克隆的虚机 `/etc/machine-id` 可能相同，导致接收端误判为同一台主机。克隆模板前执行 `rm -f /etc/machine-id && systemd-machine-id-setup` 让其重新生成唯一值；或以 `product_uuid` 为主标识。
- **容器边界**：本 agent 设计安装在物理机/虚机**宿主**上。容器内 `/proc` 未隔离，CPU/内存会反映宿主，不适用于容器内监控。
- **NTP**：`ts` 取系统时钟，部署机须配置 NTP（chrony/systemd-timesyncd）。
- **资源占用**：systemd 已设 `MemoryMax=64M`、`Nice=10`、`IOSchedulingClass=idle`、`CPUWeight=20`，agent 永不抢业务资源。
- **卸载**：`dnf remove monitor-agent` / `apt remove monitor-agent`（不删用户与配置，留给管理员）。

## 构建

```sh
cargo build --release && strip -s target/release/monitor-agent
cargo generate-rpm    # → target/generate-rpm/monitor-agent-<ver>-1.x86_64.rpm
cargo deb --no-build  # → target/debian/monitor-agent_<ver>-1_amd64.deb
```

arm64：`rustup target add aarch64-unknown-linux-gnu` 后给上述命令加 `--target aarch64-unknown-linux-gnu`。

## 资源占用对比

| 方案 | 内存 | 说明 |
|---|---|---|
| **monitor-agent** | **<15MB** | 仅采集 + 推送，无存储/ML/告警 |
| Netdata（关 ML） | ~100MB | 一体化：存储 + ML + 告警 + 可视化 |
| node_exporter | ~10–30MB | 仅采集，Prometheus 拉取模式 |

## 开发

```sh
cargo test           # 单元测试
cargo run -- --once  # 本地核对采集与 JSON
```

对接文档：[看板开发指南.md](看板开发指南.md)

License: MIT（见 [LICENSE](LICENSE)）
