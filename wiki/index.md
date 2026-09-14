# Codex Monitor 项目索引

状态：`pc-mesh-p0-p6-implemented; android-peer-validated`（2026-09-14）

这是一个面向 BOOX Poke3、NixOS 主机和 Windows 10 主机的 Codex 监控台项目。
PC 节点已实现对等发现/配对、签名状态同步、cursor/缓存和受限 interrupt。Poke3 已验证为
Android API 29 独立 P-256 节点：持久身份/信任、一次性配对、原生持久概览/额度/动态/会话仪表盘，
支持浅色、深色、电子墨水模式和节点改名。桌面 HTTP 18788 不可达时，原生面板仍可查看缓存快照。
WebView/PWA 是可选兼容查看器，不是 Android 主界面；其他写操作未开放。

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
