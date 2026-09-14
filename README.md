# Codex Monitor

面向两台开发主机和 BOOX Poke3 的 Codex 会话、账户额度监控台。

当前状态：P0/P1 对等节点同步、P2 cursor/缓存、M3.1 Poke3 PWA/Android APK、M4 受限 interrupt 和 NixOS systemd 入口已接入；SSE、其他控制命令和持久化历史仍待后续阶段。

## 目标拓扑

```text
PC-A（NixOS）                       PC-B（Windows 10）
  └─ codex-monitor-server/node  ◄──►  codex-monitor-server/node
      本机 Codex collector              本机 Codex collector

可选中央入口：仅用于 PWA 手工入口、缓存或发现 seed；当前不是状态权威或状态 relay，relay 尚未实现，仅作为未来可选扩展

BOOX Poke3
  └─ 通过 Android WebView APK 或浏览器/PWA 连接任意 PC 节点
```

PC 节点使用 LocalSend 风格的 UDP multicast/局域网广播发现和 Ed25519 配对，不依赖固定 IP；Poke3
不运行 Agent，也不维护 SSH。网络分区期间显示最后已知状态，恢复后最终收敛。

## 文档

- [项目索引](wiki/index.md)
- [去中心化自组网方案（当前）](wiki/plans/002-decentralized-mesh.md)
- [初始集中式基线（已归档）](wiki/plans/001-codex-monitor.md)
- [通用协议 v2](docs/protocol-v2.md)
- [三平台部署](docs/deployment.md)
- [Android APK 兼容性审查](docs/android-compatibility.md)
- [开发路线](wiki/roadmap.md)
- [决策日志](wiki/log.md)

## 开发环境

```bash
nix develop
cargo check --workspace
nix build .#codex-monitor-node
```

NixOS 可导入 `nixosModules.codex-monitor` 作为 systemd 服务，示例见
[`packaging/nixos/README.md`](packaging/nixos/README.md)；不会自动执行 `nixos-rebuild switch`。

在两台 PC 上运行同一个节点（自动初次连接时，两端 network ID 和配对码保持一致）：

```bash
CODEX_MONITOR_NETWORK_ID=home-lan \
CODEX_MONITOR_PAIR_CODE=Ab3dE7qK \
CODEX_MONITOR_HTTP_BIND=0.0.0.0:8788 \
cargo run -p codex-monitor-server
```

配对码为区分大小写的 8 位 ASCII 字母数字串；随机生成的码至少包含一个数字、一个大写字母和一个小写字母。
自动初次连接需要两端使用相同配对码；通过管理 API 手动配对时输入的是对端节点的配对码，两端本地配对码可以不同。
从旧版本升级时，原先的 6 位 `CODEX_MONITOR_PAIR_CODE` 必须改为 8 位；已保存的对端公钥不受配对码变更影响。

旧的 `codex-monitor-agent` 入口继续保留，用于迁移期连接不支持 peer-sync 的旧 Monitor。

身份和配对文件默认写入各平台状态目录，也可用
`CODEX_MONITOR_IDENTITY_PATH`、`CODEX_MONITOR_PAIRING_PATH`、
`CODEX_MONITOR_TRUST_PATH` 覆盖测试路径。发现会自动选择可用的真实 LAN IPv4 接口，
也可用 `CODEX_MONITOR_DISCOVERY_INTERFACE` 指定接口名或 IPv4 地址。Agent 首次配对后会 pin Monitor 公钥；若
Monitor 身份被重置，删除 trusted-monitor 文件后再配对。`CODEX_MONITOR_NETWORK_ID` 用于
隔离同一局域网中的不同逻辑网络，未设置时使用 `codex-monitor-lan`。节点会把最后成功快照
写入本地状态缓存，可用 `CODEX_MONITOR_STATE_PATH` 覆盖；重启后缓存只显示为离线/过期，
不会冒充实时数据。

如果 WLAN 阻断 multicast/broadcast，可设置
`CODEX_MONITOR_DISCOVERY_TARGETS` 为逗号分隔的主机名或 IP（可带端口；省略端口时使用 UDP
发现端口 `37991`，IPv6 使用标准 `[addr]:port` 写法）。节点每轮公告都会重新解析这些目标，
以便局域网 DNS/mDNS 主机名适应 DHCP；IP 仅作应急。目标只用于定向发送发现公告，后续仍按
现有配对流程建立 PC 直连；当前不是状态权威或状态 relay，relay 尚未实现，仅作为未来可选扩展；单目标解析或发送失败只记录警告。

节点的只读状态 API 默认只监听本机 `127.0.0.1:8788`，可用
`CODEX_MONITOR_HTTP_BIND` 覆盖。API 包含 `/api/state`、`/api/hosts` 和
`/api/accounts`，以及通用的 `/api/v1/identity`、`/api/v1/peers`、`/api/v1/state` 和
`/api/v1/health`；`/api/v1/state?since=<cursor>` 在视图未变化时返回 `304`。每台主机本地启动
`codex app-server`，只上传经过筛选的会话和账户状态；
不会上传 Codex token 或 `auth.json`。设置 `CODEX_MONITOR_READ_TOKEN` 后，所有 API 需要
`Authorization: Bearer <token>`（PWA 的“节点地址”面板可填写）；当前会话列表取最近 25 条，额度窗口按 App Server 返回的
`usedPercent` 和 `resetsAt` 展示，页面将两个窗口标为“五小时限制”和“一周限制”，不再显示
原始窗口分钟数。账户 usage 中的 `dailyUsageBuckets` 会在活动页显示为最近 14 天的日历热力图；
App Server 未返回的更早日期不补造，持久化历史留待后续阶段。

设置 `CODEX_MONITOR_CONTROL_TOKEN` 后才启用 `POST /api/v1/control`。它目前只允许经过配对节点
转发的 `interrupt`，请求需提供 `threadId`、可靠的 `turnId`、`requestId` 和短期 `expiresAt`，并用
`Authorization: Bearer <token>` 认证；不设置该 token 时控制入口保持关闭。目标节点可通过
`targetNodeId` 指定，未指定时交给本机 collector。

Monitor 同一端口提供 Poke3 页面（`/`）和静态资源；页面每 15 秒轮询 `/api/v1/state`，
采用大字号、黑白高对比布局，并在浏览器支持时使用 IndexedDB 保存最后成功快照。页面的“节点地址”可手动切换到任意
PC 或可选中央节点；在线时会从 `peers[].httpEndpoint` 和 `peers[].httpEndpoints` 学习其他已配对节点的 IP/主机名候选，当前节点不可用时自动尝试这些地址，缓存按地址隔离。
若要让 Poke3 从局域网访问，可显式设置
`CODEX_MONITOR_HTTP_BIND=0.0.0.0:8788`，再用 Windows 防火墙限制为可信 Private 网络；
读 API 在未设置 token 时不认证；控制 API 始终需要独立 token；不要暴露到公网。
也可以用 `/?endpoint=http://node:8788` 作为首次打开地址。
普通局域网 HTTP 支持在线页面，但不保证关闭后离线重开或完整 PWA 安装；需要这些能力时，请为页面和节点 API 使用 HTTPS。开发用 localhost 不代表普通局域网 HTTP 的支持情况。
PWA 节点设置面板可分别保存只读 token 和控制 token；会话在拿到活动 `turnId` 时显示受限“中断”按钮。

NixOS 是主开发环境；Windows 10 和 NixOS 都运行同一 PC 节点程序。Android 端首版使用
可安装 PWA，必要时再增加薄 APK 外壳。

## 当前边界

- 不复制 `~/.codex/auth.json` 到 Monitor 或 BOOX；
- 不把 Codex App Server 直接暴露到局域网；
- 只开放受限的 interrupt，不开放任意 shell、文件操作或自动审批；
- M1 已验证 discovery、Ed25519 challenge-response、heartbeat、重连骨架和跨主机 Windows 防火墙；
- M2 已接入 HTTP 状态 API 和 Codex App Server collector；M3/M3.1 已接入 Poke3 PWA、按钮分页和日用量热力图；M4 已接入受限 interrupt，SSE 和其他控制命令待后续阶段；
- 尚未决定项目开源许可证，暂记为 TBD。
