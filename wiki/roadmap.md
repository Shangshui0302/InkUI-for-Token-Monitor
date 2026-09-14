# 开发路线

## Android peer（已完成并经 Poke3 实机验证，2026-09-14）

- Android API 29 APK 是独立 P-256 peer，UDP 37991 发现、TCP 8787 连接；一次性配对建立持久身份信任，重启后按保存的公钥重连；
- 原生界面包含概览、额度、动态和会话持久面板。快照经 P-256 身份认证并本地持久化；HTTP 18788 不可达时已实机确认面板仍可用；
- 支持浅色、深色、电子墨水模式及节点改名；WebView 仅作为可选桌面网页兼容查看器，PWA 不是 Android 首版/主界面；
- Android peer service 不采集 Codex 数据，也不持有桌面 Codex 凭据。

## M0：工程骨架（已完成）

- Nix flake devShell；
- Rust workspace；
- 协议类型和最小测试；
- PWA 静态壳。

验收：`cargo check --workspace` 和协议 round-trip 测试通过。

## M1：发现和配对（代码已完成；实机验收待部署）

- 已实现 LocalSend 风格 UDP multicast announce（独立组播组和端口）；
- 已实现持久 Ed25519 `host_id`、公钥指纹和身份文件；
- 已实现首次配对码 + challenge-response，并在 Monitor 保存 Agent 公钥、在 Agent pin Monitor 公钥；
- 已实现 heartbeat、超时和指数退避重连；
- 已实现跨平台 LAN 接口选择，避开 Mihomo/TUN 等虚拟接口；可用
  `CODEX_MONITOR_DISCOVERY_INTERFACE` 做诊断覆盖；
- `codex-monitor-server` 已具备对等节点公告、配对和签名快照同步；旧 Agent 入口继续兼容；
  PWA 的已配对 HTTP URL 故障转移已接入，DHCP/网络重启后的持续重连仍待实机验证，hostname/.local/二维码仅在实际 WLAN 隔离时再补。

当前验收：既有本机 multicast、Windows 本机和 NixOS↔Windows 跨主机发现/配对/重连记录仍有效；
对等节点代码已完成定向验证，DHCP/网络重启后的持续重连和 hostname/.local/二维码兜底仍待实机验证。

## M2：Codex 只读监控（已完成）

- 已增加本地 `codex app-server` stdio JSONL collector；
- 已增加 thread/session 列表和状态归一化；
- 已增加账户计划、额度窗口、credits 和 usage 摘要；
- 已增加 Monitor 内存 registry、stale/offline 和同账户去重；
- 已增加 `/api/state`、`/api/hosts`、`/api/accounts`；
- 已完成 Windows/NixOS 实机验收和基本错误/断线矩阵；
- SSE 状态流留到 PWA 接入时处理。

验收已通过：Windows 和 NixOS 的状态通过同一个 Monitor API 返回，并能区分主机离线、
collector 不可用和有效的空会话；同一账户在两个主机上只显示一张聚合卡片。

## M3：网页/PWA 兼容界面（已实现；Android 主界面另见上文）

- 已完成黑白高对比布局；
- 已完成大字体和 15 秒低频轮询；
- 已完成总览、额度、活动、会话四个按钮切换页面；
- 已完成主机/账户/会话分页，减少小屏滚动；
- 已完成“五小时限制”“一周限制”标签和 `dailyUsageBuckets` 14 天热力图；
- 已完成 manifest 和静态资源离线缓存，API 请求不进入缓存；
- 已完成 Monitor 内嵌静态资源路由，页面与 API 使用同一端口；
- 已接入 IndexedDB 最后状态缓存、离线 stale 标记、手工节点地址和只读 token 设置；
- 节点在线时会学习已配对 peer 的 HTTP 入口，当前入口失联时 PWA 自动故障转移；
- 已更新为自然月日历、七色主题持久化、Eink 低刷新样式、可访问设置 modal、发现/配对/本机改名入口和按需本机配对码；
- 浏览器/PWA 仍可作为可选网页入口；Android 原生节点和仪表盘的实机验收已单独完成。持久化历史和 SSE 仍待后续阶段。

验收：Poke3 实机浏览器可访问并保持可读；桌面浏览器静态路由和 API 烟测已通过。

## M4：受限控制（代码已完成；跨设备验收待部署）

- 已完成 `interrupt`：HTTP 独立控制 token、本机 collector 路由、已连接 peer 的签名转发、短期过期、
  request id 校验和节点级防重放；
- follow-up、stop、resume、rename/archive 暂不实现；Poke3 按钮等待稳定的 `turnId` 展示；
- 控制入口默认关闭，不开放任意 shell、文件操作或自动审批。

验收：已完成协议/定向测试和 localhost 认证 smoke；跨 NixOS↔Windows/Poke3 实机控制验收留待部署。

## M5：跨平台交付（基础入口已完成）

- 已加入 Nix flake package：`nix build .#codex-monitor-node` 同时产出节点和兼容 Agent；
- 已加入 Windows `build.ps1` 与 Private 网络 `firewall.ps1`；
- 通用 HTTP/JSON 与 v2 envelope 契约见 [`docs/protocol-v2.md`](../docs/protocol-v2.md)；
- 已加入 `nixosModules.codex-monitor` systemd 模块和基于用户 env 文件的 Windows 登录启动任务；ESP32 继续直接使用通用 HTTP/JSON 契约，暂不维护未经验证的 SDK 工程。
