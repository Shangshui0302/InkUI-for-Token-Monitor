# Codex Monitor

面向两台开发主机和 BOOX Poke3 的 Codex 会话、账户额度监控台。

当前状态：M1 发现、身份和配对原型已启动；Codex 数据采集和网页 API 尚未接入。

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
`CODEX_MONITOR_TRUST_PATH` 覆盖测试路径。Agent 首次配对后会 pin Monitor 公钥；若
Monitor 身份被重置，删除 trusted-monitor 文件后再配对。

NixOS 是主开发环境。Windows 10 负责运行 Monitor 和本机 Agent；Android 端首版
使用可安装 PWA，必要时再增加薄 APK 外壳。

## 当前边界

- 不复制 `~/.codex/auth.json` 到 Monitor 或 BOOX；
- 不把 Codex App Server 直接暴露到局域网；
- 首版不做任意 shell、文件操作或自动审批；
- M1 目前只验证 discovery、Ed25519 challenge-response、heartbeat 和重连骨架；
- HTTP/SSE、Codex App Server collector、Poke3 页面和跨主机 Windows 防火墙验证待后续阶段；
- 尚未决定项目开源许可证，暂记为 TBD。
