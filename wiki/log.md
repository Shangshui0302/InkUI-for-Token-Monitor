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

## 2026-09-09：M2 只读状态链

- 新增 `codex-monitor-codex-client`，通过本机 `codex app-server --listen stdio://` 读取
  thread、账户、额度窗口和 usage 摘要；请求只读且关闭 token refresh；
- `protocol` 增加 `StateSnapshot`、账户/额度/会话类型和 `state-snapshot-v1` capability；
- Agent 仅在 Monitor 公告协商该 capability 后上报快照，Monitor 按序列号丢弃旧快照；
- Monitor 增加内存 registry 和 loopback HTTP API：`/api/state`、`/api/hosts`、`/api/accounts`；
- 同一账户在多台主机出现时按本地生成的分组 key 合并，额度不相加；保留 host offline/stale
  和 collector unavailable 状态；
- M2 已通过 NixOS 本机纵向验证和 Windows/NixOS 双主机验收；两个主机的 collector 均能返回
  会话、计划和额度窗口，Monitor API 保留 stale/offline 状态并合并同一账户。

## 2026-09-09：M3 Poke3 PWA

- Poke3 首版采用原生 HTML/CSS/JS 和同端口静态资源，不引入 React/Vue 或 Node 构建链；
- 页面每 15 秒轮询 `/api/state`，保留手动刷新和断线/过期提示；SSE 延后到确有功耗或
  延迟需求时再加入；
- Service Worker 只缓存静态壳，`/api/*` 永不缓存，避免额度和会话显示旧数据；
- Monitor 默认继续 loopback，用户显式 LAN 绑定时需使用可信 Private 网络；M3 不开放写入或
  未认证控制接口。

## 2026-09-09：M3.1 小屏活动视图

- `account/usage/read` 的 `dailyUsageBuckets` 已进入受限 `AccountState`，只接受严格日期和
  非负 token，并限制最多 400 个 bucket；不使用日用量反推五小时/一周额度；
- Poke3 页面改为“总览 / 额度 / 活动 / 会话”按钮切换，账户、主机和会话使用分页；
- 额度窗口显示为“五小时限制”和“一周限制”，隐藏 `windowDurationMins` 文本；
- 活动页默认显示最近 14 天热力图，具体日期和 token 同时显示；App Server 未返回的历史不
  伪造，持久化历史留到后续阶段。
