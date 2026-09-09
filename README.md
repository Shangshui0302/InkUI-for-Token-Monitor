# Codex Monitor

面向两台开发主机和 BOOX Poke3 的 Codex 会话、账户额度监控台。

当前状态：仓库初始化和方案固化阶段，业务功能尚未实现。

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

NixOS 是主开发环境。Windows 10 负责运行 Monitor 和本机 Agent；Android 端首版
使用可安装 PWA，必要时再增加薄 APK 外壳。

## 当前边界

- 不复制 `~/.codex/auth.json` 到 Monitor 或 BOOX；
- 不把 Codex App Server 直接暴露到局域网；
- 首版不做任意 shell、文件操作或自动审批；
- 尚未决定项目开源许可证，暂记为 TBD。
