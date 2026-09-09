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

## 2026-09-09：开启 M1

- `main` 已从 bootstrap 分支快进到提交 `014b4f9`；M1 在独立分支 `codex/m1-discovery` 开发；
- M1 传输先采用换行分隔 JSON over TCP，保留后续切换 WebSocket 的 payload 兼容边界；
- Monitor 广播 `239.255.77.77:37991`，控制链路监听 TCP `8787`；公告每 5 秒刷新、20 秒过期；
- Agent 使用出站连接，首次用配对码，之后用双方持久化的公钥；Agent 会 pin Monitor 身份，heartbeat 超时后重新发现；
- 本机联调已验证发现、配对和 heartbeat；跨主机 DHCP/Windows 防火墙验证留在 M1 收尾。

## 2026-09-09：M1.1 组播接口选择

- 实测 Mihomo/TUN 会在两台主机上抢占 `224.0.0.0/4`，导致未指定接口的组播发现偏离 WLAN；
- `if-addrs` 枚举接口，自动跳过 loopback、点对点、虚拟/保留隧道地址，并优先真实私有 LAN；
- `socket2` 为 Monitor 设置 `IP_MULTICAST_IF`，Agent 在选定接口加入组播；
- `CODEX_MONITOR_DISCOVERY_INTERFACE` 支持按接口名或 IPv4 地址覆盖自动选择；
- Windows 本机发现/配对，以及 NixOS↔Windows 跨主机发现、首次配对和无配对码重连已通过；
- NixOS 防火墙通过声明式 UDP `37991` 放行，Windows 端规则限定为 Private 网络。
