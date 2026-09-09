# 决策日志

## 2026-09-09：初始化方案

- 中央 Monitor 定为 Windows 10 主机，原因是用户希望该设备承担中心角色；
- NixOS 主机运行远程 Agent；
- BOOX Poke3 首版使用 Android 浏览器/PWA，不先开发原生 App；
- 主机发现采用 LocalSend 风格 UDP multicast，但使用独立服务名、端口和消息格式；
- Agent 主动连接 Monitor，避免固定 IP 和日常 SSH；
- 主开发环境选择 NixOS 宿主机，通过项目 flake 提供 Rust 工具链；
- 首版不复制 `auth.json`、不暴露任意 shell、不自动审批；
- 许可证、是否需要 APK、Windows Service 形式仍为 `TBD`。
