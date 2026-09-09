# Codex Monitor 项目索引

状态：`m3.1-in-progress`（2026-09-09）

这是一个面向 BOOX Poke3、NixOS 主机和 Windows 10 主机的 Codex 监控台项目。
当前仓库已包含 M1 发现/配对、M2 只读状态采集/API 和 M3.1 Poke3 PWA（按钮分页、额度标签和
日用量热力图）；会话控制仍未接入。

## 入口

- [完整方案](plans/001-codex-monitor.md)
- [开发路线](roadmap.md)
- [决策日志](log.md)

## 文档规则

- 方案和长期决策以 `wiki/` 为准；
- 代码行为需要在实现后补充验证证据；
- build、activation、runtime 状态分开记录；
- 未确认的行为写为 `Unknown`，不把设计意图写成已实现事实。
