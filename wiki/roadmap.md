# 开发路线

## M0：工程骨架（已完成）

- Nix flake devShell；
- Rust workspace；
- 协议类型和最小测试；
- PWA 静态壳。

验收：`cargo check --workspace` 和协议 round-trip 测试通过。

## M1：发现和配对（进行中）

- 已实现 LocalSend 风格 UDP multicast announce（独立组播组和端口）；
- 已实现持久 Ed25519 `host_id`、公钥指纹和身份文件；
- 已实现首次配对码 + challenge-response，并在 Monitor 保存 Agent 公钥、在 Agent pin Monitor 公钥；
- 已实现 heartbeat、超时和指数退避重连；
- 已实现跨平台 LAN 接口选择，避开 Mihomo/TUN 等虚拟接口；可用
  `CODEX_MONITOR_DISCOVERY_INTERFACE` 做诊断覆盖；
- 待验证 DHCP/网络重启后的跨主机重连；hostname/URL 兜底仍待实现。

当前验收：本机 multicast、Windows 本机和 NixOS↔Windows 跨主机发现/配对/重连已通过；
DHCP/网络重启后的持续重连、hostname/URL 兜底仍待实现。

## M2：Codex 只读监控

- thread/session 列表；
- active、idle、waiting、error；
- 最近输出摘要；
- 账户计划和 5 小时/周额度；
- credits、last-seen、stale 状态。

验收：Windows 和 NixOS 同时显示在一张监控页。

## M3：Poke3 PWA

- 黑白高对比布局；
- 大字体和低频刷新；
- 主机、会话、账户卡片；
- manifest 和离线缓存。

验收：Poke3 实机浏览器可访问并保持可读。

## M4：受限控制

- follow-up；
- interrupt；
- stop；
- resume；
- rename/archive。

验收：控制命令具有 capability 校验和明确错误状态。

## M5：跨平台交付

- NixOS agent package；
- Windows server/agent `.exe`；
- Android PWA；
- 如 PWA 发现体验不足，再增加薄 APK shell。
