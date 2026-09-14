# Codex Monitor 完整方案

状态：`baseline-archived`

日期：2026-09-09

> 本文保留最初的 Windows Monitor + NixOS Agent 基线。去中心化节点方案已在
> [`002-decentralized-mesh.md`](002-decentralized-mesh.md) 更新，后续实现以新方案为准。
>
> **历史档案：** 下文描述的是 2026-09-09 的初始方案和当时的目标/取舍，不是现行架构声明。
> 特别是 Android 浏览器/PWA 首版、不建设原生 Android 业务层、单一 Windows Monitor 等内容均已被后续实现取代；
> 当前 Android 是独立 P-256 peer 和原生持久仪表盘，详见 `002-decentralized-mesh.md` 与 `../../docs/android-compatibility.md`。

## 1. 目标和边界

### 目标

在不购买额外中央设备、无需固定 IP、无需让 BOOX 维护 SSH 的前提下，提供：

- 两台开发主机的 Codex 会话状态；
- ChatGPT/Codex 账户计划、窗口额度、credits 和 reset 时间；
- 简单会话控制；
- BOOX Poke3 可读的低刷新监控界面；
- NixOS、Windows 10、Android 三个平台的可运行交付。

### 已确认的角色

```text
Windows 10 主机 B
  ├─ codex-monitor-server
  └─ 本机 Codex collector

NixOS 主机 A
  └─ codex-monitor-agent
       └─ 主动连接 Windows Monitor

BOOX Poke3
  └─ Android 浏览器/PWA
```

中央 Monitor 是逻辑角色，不是新增硬件。默认放在 Windows 10；如果未来更换常开主机，
只迁移 Monitor，不改变 Agent 与 Poke3 协议。

### 非目标

- 不做完整 AMux 替代品或 agent fleet 编排；
- 不复制 `auth.json` 到中央服务或 BOOX；
- 不把 Codex App Server 直接暴露到局域网；
- 不在首版开放任意 shell、文件操作、自动 approval 或 `danger-full-access`；
- 不在首版建设原生 Android 业务层；
- 不在首版建设历史数据库，除非需求被确认。

## 2. 关键证据

### Codex 接口

官方 Codex App Server 提供双向 JSON-RPC，可用于会话、事件、控制和账户信息：

- `account/read`；
- `account/rateLimits/read`；
- `account/usage/read`；
- thread/session 列表和读取；
- turn follow-up、interrupt 等操作。

本机 `codex-cli 0.151.0` 已做只读验证，以上账户和会话信息可以从本地 App Server
取得。M2 collector 使用默认 stdio JSONL，不把 App Server 监听端口暴露到局域网。官方将
App Server 定位为深度 UI 集成接口，见
[Codex App Server 文档](https://developers.openai.com/codex/app-server/) 和
[OpenAI Codex harness 说明](https://openai.com/index/unlocking-the-codex-harness/)。

### LocalSend 发现机制

LocalSend 的核心不是 mDNS，而是自定义 UDP multicast 加 HTTP(S) 注册：默认组播地址为
`224.0.0.167:53317`，设备广播自身信息，收到广播的设备通过
`POST /api/localsend/v2/register` 回调；multicast 失败时可扫描局部地址作为兜底。
设备用 fingerprint 避免自发现并记住身份。参考
[LocalSend Protocol](https://github.com/localsend/protocol) 和
[LocalSend 源码说明](https://github.com/localsend/localsend/blob/main/AGENTS.md)。

本项目只借鉴这个模式，不复用 LocalSend 端口、组播地址或消息格式。

### BOOX 约束

Poke3 是 Android 10、6 英寸、300dpi、2GB RAM 设备，适合大字号、黑白、高对比、
低频刷新网页，不适合持续滚动日志和动画。[BOOX Poke3 官方资料](https://shop.boox.com/blogs/news/poke3-valentine-edition)

## 3. 总体架构

```text
                         ┌─────────────────────────────┐
                         │ BOOX Poke3 Android Browser  │
                         │ responsive PWA              │
                         └──────────────┬──────────────┘
                                        │ HTTP(S) + SSE
                                        ▼
┌────────────────────────────────────────────────────────────────┐
│ Windows 10 Monitor Host                                        │
│                                                                │
│  monitor-server.exe                                            │
│   ├─ HTTP API + SSE                                            │
│   ├─ host registry / pairing                                   │
│   ├─ account/session aggregation                                │
│   ├─ LocalSend-style discovery responder                       │
│   └─ static PWA                                                 │
│                                                                │
│  local Codex collector                                         │
│   └─ codex app-server --listen stdio://                        │
└───────────────────────────▲────────────────────────────────────┘
                            │ outbound control link
                            │ dynamic IP independent
┌───────────────────────────┴────────────────────────────────────┐
│ NixOS Agent Host                                                │
│  codex-monitor-agent                                            │
│   ├─ multicast discovery listener                               │
│   ├─ persistent host identity                                  │
│   ├─ local codex app-server client                             │
│   └─ reconnect/backoff                                          │
└────────────────────────────────────────────────────────────────┘
```

### Monitor 进程边界

Windows 上首版运行两个逻辑组件：

1. `monitor-server.exe`：网页、聚合、配对、Agent 连接；
2. 本机 collector：连接 Windows 本机的 Codex App Server。

实现上可以由一个 Rust 二进制提供 `monitor` 和 `agent` 两个子命令，Windows 使用两个
进程或一个组合启动器；先选择更容易测试的两个进程。

## 4. 发现、配对和重连

### 4.1 Monitor 广播

Monitor 在固定端口发布服务公告。公告只包含：

```json
{
  "service": "codex-monitor",
  "version": 1,
  "host_id": "stable-monitor-id",
  "alias": "Windows Monitor",
  "port": 8787,
  "public_key": "...",
  "capabilities": ["sessions", "control", "usage"]
}
```

不放入 token、账户邮箱、Codex 输出或 `auth.json` 内容。

### 4.2 Agent 连接

NixOS Agent：

1. 启动时加载本地稳定 `host_id` 和密钥；
2. 监听 Monitor 广播；
3. 首次连接用 Monitor 显示的配对码确认；
4. 之后以公钥/host ID 重新连接；
5. 建立出站控制长连接（M1 为换行分隔 JSON over TCP，M2 再升级为 WebSocket/HTTPS）；
6. 断线后指数退避并重新发现；
7. IP 改变不影响配对关系。

### 4.3 Poke3 地址

Monitor 同时发布稳定的人类可用地址，例如：

```text
http://codex-monitor.local
```

PWA 不直接监听 UDP multicast，因此 Poke3 端使用稳定 hostname、路由器本地 DNS 或
一次性 URL 作为入口。若 Poke3 浏览器无法解析 `.local`，再增加原生 APK 作为发现外壳，
而不改后端协议。

### 4.4 降级路径

按优先级：

1. multicast 自动发现；
2. `.local` hostname；
3. 一次性 pairing URL/二维码；
4. 手工输入当前地址。

不把全网 IP 扫描作为首选方案，避免慢、误报和防火墙噪声。

### 4.5 M1 已落地边界

- `crates/protocol`：公告和控制链路消息、长度/TTL/身份校验、Ed25519 身份和签名；
- `crates/monitor`：UDP 公告、TCP 握手、首次配对码、公钥持久化、heartbeat 超时；
- `crates/agent`：监听公告、动态地址连接、challenge-response、Monitor 公钥 pin、heartbeat 和指数退避；
- 运行时状态只写入各平台状态目录，私钥不进入网络消息；
- 当前不读取 Codex、不提供 HTTP/SSE；跨主机 Windows 验收已完成，Codex 采集仍未接入。

### 4.6 M2 已落地边界

- `crates/codex-client`：启动本机 `codex app-server --listen stdio://`，执行只读的
  `thread/list`、`account/read`、`account/rateLimits/read` 和 `account/usage/read`；
- `crates/protocol`：增加受限 `StateSnapshot`，带序列号、采集时间、collector 状态和长度校验；
- `crates/agent`：仅在 Monitor 公告声明 `state-snapshot-v1` 时上报快照；
- `crates/monitor`：保存主机快照，保留 offline/stale 状态，按账户分组 key 去重，并提供本机
  HTTP JSON API（默认 `127.0.0.1:8788`）；
- 不上传原始邮箱、token、`auth.json`、完整 transcript，也不开放控制 RPC；
- 会话列表当前取最近 25 条，`notLoaded` 映射为 `unknown`，不冒充 `idle`。

当前 collector 通过独立的只读 App Server 读取持久化 thread；如果另一个 CLI/IDE 进程
尚未把 thread 加载到该 App Server，官方返回的运行时状态会是 `notLoaded`，页面显示
`unknown`。这避免把“无法确认”误报成空闲；共享 daemon/control-socket 的实时状态接入
留到后续 M2.x。

### 4.7 M3 Poke3 PWA 边界

- `web/` 使用原生 HTML/CSS/JS，不引入前端构建依赖；页面按 15 秒间隔轮询
  `/api/state`，保留手动刷新和明确的断线提示；
- Monitor 将页面和 Service Worker 静态资源编译进服务器二进制，`/`、`/app.js`、
  `/styles.css`、`/manifest.webmanifest` 和 `/sw.js` 与 JSON API 共用端口；
- Service Worker 只缓存静态壳，所有 `/api/*` 请求直连网络，离线时仅恢复页面壳并显示状态
  可能过期；
- 布局采用黑白高对比、大字号和无动画设计，使用“总览 / 额度 / 活动 / 会话”按钮切换，
  主机、账户和会话使用分页以减少小屏滚动；
- 额度页将 primary/secondary 标为“五小时限制”和“一周限制”，隐藏原始窗口分钟数；
- 活动页读取 `account/usage/read` 的 `dailyUsageBuckets`，以最近 14 天的日历热力图显示
  每日 token，并保留日期与 token 文本，未返回的更早历史不补造；
- HTTP 默认仍绑定 loopback；LAN 绑定需要用户显式设置并配合 Windows Private 防火墙，M3
  不提供认证或写操作。

M3.1 验证分为桌面静态路由/API 烟测、热力图数据解析测试和 Poke3 实机手工验收。SSE、会话控制、历史数据库和
Poke3 自动发现继续留在后续阶段。

## 5. Codex 数据流

Agent 负责本地凭据边界，Monitor 只接收必要的已筛选数据。

### 会话数据

- host ID、主机别名；
- thread ID、标题、状态；
- 最近一项输出摘要；
- start/finish/error 时间；
- token activity（作为会话统计，不冒充账户额度）。

### 账户数据

- `planType`；
- primary/secondary rate-limit window；
- `usedPercent`；
- `windowDurationMins`；
- `resetsAt`；
- credits；
- `dailyUsageBuckets`（日期和 token，用于活动热力图）；
- `lastUpdated` 和 stale/error 状态。

同一个账户在两台主机登录时，Monitor 按账户身份合并为一张卡片，不将两个
`usedPercent` 相加。若两台机器是不同账户，则显示不同账户卡片。

## 6. API 设计草案

### 浏览器 API（M2/M3 已实现只读部分）

```text
GET  /                         # Poke3 PWA
GET  /app.js
GET  /styles.css
GET  /manifest.webmanifest
GET  /sw.js
GET  /api/state
GET  /api/hosts
GET  /api/accounts
```

```text
GET  /api/events                 # M3/PWA 阶段再加入 SSE
POST /api/hosts/:host/threads/:thread/turn
POST /api/hosts/:host/threads/:thread/interrupt
POST /api/hosts/:host/threads/:thread/stop
POST /api/hosts/:host/threads/:thread/resume
POST /api/hosts/:host/threads/:thread/rename
```

### Agent 链路消息

```text
AgentHello
PairChallenge
PairAccepted
Heartbeat
StateSnapshot
StateEvent
Command
CommandResult
```

所有命令都必须经过 capability 检查；默认只读。

## 7. 技术栈

- Rust stable：Agent 和 Monitor；
- Tokio：异步运行时；
- Axum：HTTP、SSE、WebSocket；
- Serde/Serde JSON：协议与 Codex JSON-RPC；
- `socket2`：跨平台 UDP multicast；
- Rustls：避免 OpenSSL 带来的 Windows 交付差异；
- Ed25519 公钥：主机身份与配对；
- 原生 HTML/CSS/JavaScript：Poke3 PWA；
- Nix flake：开发环境和 NixOS 打包。

首版不引入 React、Flutter、Electron、Docker 或额外数据库。

## 8. 仓库结构

```text
codex-monitor-UI/
├── flake.nix / flake.lock
├── Cargo.toml / Cargo.lock
├── crates/
│   ├── protocol/src/
│   │   ├── lib.rs
│   │   ├── messages.rs
│   │   ├── discovery.rs
│   │   └── capabilities.rs
│   ├── agent/src/
│   │   ├── main.rs
│   │   ├── codex_client.rs
│   │   ├── monitor_link.rs
│   │   ├── discovery.rs
│   │   └── pairing.rs
│   └── monitor/src/
│       ├── main.rs
│       ├── registry.rs
│       ├── discovery.rs
│       ├── api.rs
│       ├── events.rs
│       ├── auth.rs
│       └── state.rs
├── web/
│   ├── index.html
│   ├── app.js
│   ├── styles.css
│   ├── manifest.webmanifest
│   └── sw.js
├── packaging/
│   ├── nix/
│   │   ├── package.nix
│   │   └── module.nix
│   └── windows/
│       ├── install-monitor.ps1
│       ├── install-agent.ps1
│       └── firewall-rules.ps1
├── tests/
└── wiki/
```

运行时状态不进入 Git：

- Linux：`$XDG_STATE_HOME/codex-monitor/`；
- Windows：`%LOCALAPPDATA%\\CodexMonitor\\`；
- Monitor 保存 Agent 公钥，Agent 保存已信任的 Monitor 公钥；
- 凭据和身份私钥只留在各自主机。

## 9. 平台交付

### NixOS

- `codex-monitor-agent` package；
- 可选 systemd service module；
- firewall 只开放 Agent 所需的本地发现/出站链路；
- 用 `nix develop`、`nix flake check`、`nix build` 分层验证；
- 不自动执行 `nixos-rebuild switch`。

### Windows 10

- `codex-monitor-server.exe`；
- 本机 `codex-monitor-agent.exe`；
- Private Network 防火墙规则；
- 首版用计划任务/登录启动；
- 稳定后再决定是否包装为 Windows Service。

### Android/Poke3

- 首版是可安装 PWA；
- 静态资源由 Windows Monitor 提供；
- 黑白高对比、大按钮、低频批量刷新；
- 如果 `.local` 解析或后台行为不稳定，再增加薄 APK shell；
- APK 只负责发现和加载同一套 Web UI。

## 10. 开发计划和验收

### M0：骨架

交付 Nix devShell、Rust workspace、协议类型、PWA 静态壳。

验收：`cargo check --workspace`、协议 JSON round-trip 测试通过。

### M1：发现和配对（进行中）

已交付 multicast announce、稳定 host ID、首次配对、challenge-response、heartbeat 和自动重连原型。

本机 multicast、Windows 本机和 NixOS↔Windows 跨主机发现/配对/无配对码重连已通过；
改变 DHCP 租约、重启网络的持续重连，以及 hostname/URL 兜底仍待验证。

当前验证证据：`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
`cargo test --workspace`、PWA 语法检查和 `nix flake check path:. --no-build` 均通过；
本机已验证“首次配对 → Monitor 重启 → Agent 无配对码回连”。

### M2：Codex 只读

交付两台主机的 thread 状态、最近输出、账户计划、5 小时/周额度和 credits。

验收：Windows 和 NixOS 同时显示在同一页面，账户额度不重复计算。

### M3：Poke3 PWA

交付响应式页面、manifest、离线壳、手动刷新和墨水屏布局。

验收：Poke3 实机可访问、可读、断线后能恢复。

### M4：受限控制

交付 follow-up、interrupt、stop、resume、rename/archive。

验收：每个命令都有权限检查、确认状态和失败反馈。

### M5：跨平台打包

交付 NixOS agent、Windows server/agent、Android PWA；如有需要再交付 APK。

验收：三平台运行矩阵、动态 IP、Monitor 重启、Agent 重启和账户接口异常均有记录。

## 11. 风险和处理

| 风险 | 处理 |
|---|---|
| multicast 被 Wi-Fi/AP 隔离 | hostname、配对 URL、手工地址兜底 |
| Windows 防火墙拦截 | 安装脚本创建 Private Network 规则 |
| Windows 休眠 | 记录为中央主机可用性取舍；不做自动故障转移 |
| Codex RPC 字段变化 | 允许未知字段，记录 CLI 版本，缺字段显示未报告 |
| Poke3 刷新过慢 | 摘要卡片、批量更新、无实时日志动画 |
| 局域网伪造广播 | 配对码、公钥指纹、capability 校验 |
| 控制命令误操作 | 默认只读，按钮确认，暂不开放高风险操作 |

## 12. 需要在实现前确认的事项

- Android PWA 是否作为第三个平台的首版交付；
- 是否必须在第一版生成 APK；
- Windows 主机是否可以保持 Monitor 进程运行；
- 两台主机是否登录同一个 ChatGPT 账户；
- 项目许可证选择（当前为 `TBD`）。
