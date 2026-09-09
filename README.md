# Codex Monitor

面向两台开发主机和 BOOX Poke3 的 Codex 会话、账户额度监控台。

当前状态：M3.1 Poke3 只读 PWA 已接入；SSE、会话控制和持久化历史仍待后续阶段。

## 目标拓扑

```text
Windows 10 主机
  ├─ codex-monitor-server
  └─ 本机 Codex collector

NixOS 主机
  └─ codex-monitor-agent（主动连接 Windows Monitor）

BOOX Poke3
  └─ Android 浏览器/PWA
```

主机使用 LocalSend 风格的 UDP multicast 发现机制，Agent 通过出站连接回到
Monitor，因此不依赖固定 IP，也不让 Poke3 维护 SSH。

## 文档

- [项目索引](wiki/index.md)
- [完整方案](wiki/plans/001-codex-monitor.md)
- [开发路线](wiki/roadmap.md)
- [决策日志](wiki/log.md)

## 开发环境

```bash
nix develop
cargo check --workspace
```

在两个终端做本机联调（配对码需要一致）：

```bash
CODEX_MONITOR_PAIR_CODE=246810 cargo run -p codex-monitor-server
CODEX_MONITOR_PAIR_CODE=246810 cargo run -p codex-monitor-agent
```

身份和配对文件默认写入各平台状态目录，也可用
`CODEX_MONITOR_IDENTITY_PATH`、`CODEX_MONITOR_PAIRING_PATH`、
`CODEX_MONITOR_TRUST_PATH` 覆盖测试路径。发现会自动选择可用的真实 LAN IPv4 接口，
也可用 `CODEX_MONITOR_DISCOVERY_INTERFACE` 指定接口名或 IPv4 地址。Agent 首次配对后会 pin Monitor 公钥；若
Monitor 身份被重置，删除 trusted-monitor 文件后再配对。

Monitor 的只读状态 API 默认只监听本机 `127.0.0.1:8788`，可用
`CODEX_MONITOR_HTTP_BIND` 覆盖。API 包含 `/api/state`、`/api/hosts` 和
`/api/accounts`。每台主机本地启动 `codex app-server`，只上传经过筛选的会话和账户状态；
不会上传 token 或 `auth.json`。当前会话列表取最近 25 条，额度窗口按 App Server 返回的
`usedPercent` 和 `resetsAt` 展示，页面将两个窗口标为“五小时限制”和“一周限制”，不再显示
原始窗口分钟数。账户 usage 中的 `dailyUsageBuckets` 会在活动页显示为最近 14 天的日历热力图；
App Server 未返回的更早日期不补造，持久化历史留待后续阶段。

Monitor 同一端口提供 Poke3 页面（`/`）和静态资源；页面每 15 秒轮询 `/api/state`，
采用大字号、黑白高对比布局。若要让 Poke3 从局域网访问，可显式设置
`CODEX_MONITOR_HTTP_BIND=0.0.0.0:8788`，再用 Windows 防火墙限制为可信 Private 网络；
M3 API 仍是只读且未认证，不要暴露到公网。

NixOS 是主开发环境。Windows 10 负责运行 Monitor 和本机 Agent；Android 端首版
使用可安装 PWA，必要时再增加薄 APK 外壳。

## 当前边界

- 不复制 `~/.codex/auth.json` 到 Monitor 或 BOOX；
- 不把 Codex App Server 直接暴露到局域网；
- 首版不做任意 shell、文件操作或自动审批；
- M1 已验证 discovery、Ed25519 challenge-response、heartbeat、重连骨架和跨主机 Windows 防火墙；
- M2 已接入 HTTP 状态 API 和 Codex App Server collector；M3/M3.1 已接入 Poke3 PWA、按钮分页和日用量热力图；SSE 和控制命令待后续阶段；
- 尚未决定项目开源许可证，暂记为 TBD。
