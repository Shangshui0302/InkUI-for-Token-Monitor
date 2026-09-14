# Codex Monitor 去中心化自组网方案

状态：`p0-p6-implemented; deployment-acceptance-pending`（2026-09-10）

## 目标

两台或多台 PC 在同一局域网内自动发现、配对、互相同步本机 Codex 状态；任意节点提供
相同的 `NetworkView`，Poke3 连接哪台 PC 都能看到同一份最终一致视图。中央节点只是可选的
入口、缓存或发现 seed；当前不是状态权威或状态 relay，relay 尚未实现，仅作为未来可选扩展，
也不需要购买第三台设备。

Poke3/ESP32 是只读客户端，不运行 Codex agent、不持有 Codex 凭据；会话控制只由带独立
控制 token 的 PWA 发起，并路由到拥有目标会话的 PC。

## 拓扑和角色

```text
PC-A (NixOS)  ◄── multicast discovery + Ed25519 paired TCP ──►  PC-B (Windows 10)
     │                                                         │
     └────────────── same NetworkView / HTTP JSON ────────────┘
                              ▲
                    Poke3 / ESP32 / browser
```

- 每台 PC 运行同一个 `codex-monitor-node`（兼容命令名仍为 `codex-monitor-server`），并在本机
  启动 `codex app-server --listen stdio://` collector；旧 `codex-monitor-agent` 入口保留作迁移兼容。
- `networkId` 隔离同一 LAN 上的不同逻辑网络；`hostId` 来自持久 Ed25519 公钥，不依赖 DHCP 地址。
- 节点按 `hostId` 字典序只让一端主动拨号，避免双连接；被动端也注册反向控制路由。
- 可选中央节点无需特殊业务代码：任意已配对 PC 都可作为 Poke3 手工配置的入口；multicast/broadcast
  受限时可通过 `CODEX_MONITOR_DISCOVERY_TARGETS` 作为发现 seed，但 PC 仍建立直连，其离线不
  阻断已配对 PC 的同步；relay 尚未实现，未来是否增加需单独评估。

## 发现、配对和重连

1. 节点以 TTL=1 的 UDP multicast `239.255.77.77:37991` 广播服务、TCP 端口、公钥、能力和
   `networkId`；同时发往 `255.255.255.255:37991` 作为 multicast 被 WLAN 禁止时的轻量兜底；
   接口选择跳过 Mihomo/TUN 等虚拟网卡。
2. 首次 TCP 握手使用短期配对码和 Ed25519 challenge-response，双方保存对端公钥；之后只按
   公钥/host ID 重连，不要求重新输入 IP 或 SSH。
3. heartbeat 和超时驱动断开；下一次公告使用新的源地址重连。multicast/broadcast 被 AP 隔离时，
   可用 `CODEX_MONITOR_DISCOVERY_TARGETS` 定向发送公告（端口可省略，默认 UDP 发现端口
   `37991`）；优先填写能随 DHCP 更新的局域网 DNS/mDNS 主机名，IP 仅作应急。PWA 仍保留手工
   HTTP 入口和已学习 endpoint failover；二维码/
   一次性 URL 尚未实现，不把它们写成当前能力。
4. 未知公钥、错误 network ID、错误签名和过期信封直接拒绝；撤销通过删除本地配对记录完成。

## 状态一致性

每个来源节点只发布自己采集的 `StateSnapshot`。快照含 `sourceNodeId`、`sequence`、
`capturedAt`、collector 状态、会话和账户摘要；接收节点按来源保存最大 sequence，拒绝重复、
乱序和不匹配的 host/source。所有 PC 用 BTree 稳定排序、相同账户聚合和相同 stale/offline 规则，
因此在连接恢复后最终收敛。

断网期间不承诺强一致：最后快照保留，但界面明确显示 `stale` 或 `offline`。重启恢复的缓存
同样只显示为离线，不冒充实时数据。当前节点对每个其他已发现节点建立直接连接，满足本项目规模的
全量同步；以后规模需要增大时再增加带来源签名的 state bundle/anti-entropy，不提前引入 CRDT。
缓存启动加载还会校验 `sourceNodeId` 和当前 `networkId`，切换逻辑网络时不会复用旧网络快照。

## 协议

- 传输：换行分隔 JSON over TCP，旧 v1 Agent 消息继续可读；新 peer 状态和控制使用 v2
  `MessageEnvelope`，具体字段和签名规则见 [`docs/protocol-v2.md`](../../docs/protocol-v2.md)。
- 能力：`state-snapshot-v1`、`peer-sync-v1`、`control`；未协商能力的消息不执行。
- 客户端 API：

  ```text
  GET /api/v1/identity
  GET /api/v1/peers
  GET /api/v1/state
  GET /api/v1/state?since=<cursor>
  GET /api/v1/health
  ```

- `/api/v1/state` 未变化时返回 `304`；只读 token 通过 `CODEX_MONITOR_READ_TOKEN` 开启。
- 当前写接口只有 `POST /api/v1/control` 的 `interrupt`，需要独立的
  `CODEX_MONITOR_CONTROL_TOKEN`、目标节点、thread/turn ID、request ID 和不超过 120 秒的过期时间。
  没有可靠 `turnId` 时 PWA 不显示可执行按钮。

## 客户端和平台交付

- Poke3：原生 HTML/CSS/JS PWA，四个按钮分页、五小时/一周额度标签、14 天热力图、IndexedDB
  最后快照和离线提示；从 `peers[].httpEndpoint(s)` 学习已配对节点并在入口失联时自动故障转移；
  不监听 multicast、不运行 agent。
- ESP32：只实现 HTTP GET、JSON 子集和 `since` cursor；缓存最后快照，显示更新时间/stale；不需要
  Rust 或 peer 私钥。
- NixOS：`nix build .#codex-monitor-node` 生成节点和兼容 agent；系统服务/防火墙仍由用户的
  NixOS 配置声明并手动 activation。
- Windows 10：`packaging/windows/build.ps1` 生成两个 `.exe`；`run-node.ps1` 负责设置 network ID、
  配对码和 token；`firewall.ps1` 只为 Private 网络固定开放 TCP `8787`、UDP `37991` 和默认 HTTP
  TCP `8788`（自定义 HTTP 端口时需同步 `-HttpPort`）。

## 当前阶段与后续

- P0/P1：协议、身份、发现、配对、双向状态同步和兼容 Agent 已接入。
- P2/P3：cursor/304、状态缓存、Poke3 离线缓存和任意节点手工切换已接入。
- P5/P6：只读/控制 token、签名控制、双向 interrupt 路由已接入；其他写操作暂不开放。
- 自动/本地已验证：Windows 安全部署与 x64 release 产物（server/agent）、Windows localhost
  HTTP 200 与 Codex CLI 定位、NixOS 实际 collector 的 sessions/account/300 与 10080 分钟窗口
  及 daily usage、Podman 双节点协议链路和定向 discovery 发送单测。
- 手工/外部验收仍待完成：NixOS↔Windows 真实 mesh（本轮因验收编排/启动路径未形成证据，不能归因
  为程序失败）、DHCP/网络重启、Windows HTTP `8788` 入站规则、Poke3 加网/安装/断线恢复和控制。
- 明日最小顺序：用临时状态启动双方并完成真实 mesh；由管理员添加 Windows HTTP `8788` 规则；再让
  Poke3 连接任一节点。具体 DHCP 地址不写入方案，优先使用局域网 DNS/mDNS 主机名。
- 历史数据库、SSE、MQTT/WebSocket、原生 APK 和任意 shell/文件操作仍保持需求驱动；relay 仍未实现。
