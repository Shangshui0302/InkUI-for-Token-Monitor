# Codex Monitor 项目索引

状态：`decentralized-p0-p6-in-progress`（2026-09-10）

这是一个面向 BOOX Poke3、NixOS 主机和 Windows 10 主机的 Codex 监控台项目。
当前仓库已包含对等节点发现/配对、签名状态同步、P2 cursor/缓存、M3.1 Poke3 PWA（按钮分页、
额度标签、日用量热力图、离线缓存）和可选只读 token；受限 interrupt 控制已接入，其他写操作未开放。

## 入口

- [去中心化自组网方案（当前）](plans/002-decentralized-mesh.md)
- [初始集中式基线（已归档）](plans/001-codex-monitor.md)
- [通用协议 v2](../docs/protocol-v2.md)
- [三平台部署](../docs/deployment.md)
- [开发路线](roadmap.md)
- [决策日志](log.md)

## 文档规则

- 方案和长期决策以 `wiki/` 为准；
- 代码行为需要在实现后补充验证证据；
- build、activation、runtime 状态分开记录；
- 未确认的行为写为 `Unknown`，不把设计意图写成已实现事实。
