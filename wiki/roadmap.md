# 开发路线

## M0：工程骨架

- Nix flake devShell；
- Rust workspace；
- 协议类型和最小测试；
- PWA 静态壳。

验收：`cargo check --workspace` 和协议 round-trip 测试通过。

## M1：发现和配对

- LocalSend 风格 UDP multicast announce；
- 稳定 `host_id` 和公钥；
- 一次性配对；
- DHCP 地址变化后的自动重连；
- hostname/URL 兜底。

验收：改变租约或重启网络后无需修改配置即可重连。

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
