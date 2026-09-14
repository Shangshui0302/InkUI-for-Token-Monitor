# Codex Monitor 部署

这套部署不需要固定 IP，也不需要 Poke3 运行 SSH。两台 PC 都运行同一个节点程序；Poke3
则运行独立 Android peer，通过 UDP 发现和已配对 TCP 连接接收认证状态快照。原生仪表盘将快照持久化，桌面 HTTP 不可达时仍可查看最后同步的数据。

## 端口和配置

| 用途 | 默认端口 | 方向 |
| --- | ---: | --- |
| 节点发现 | UDP 37991 | 局域网广播/multicast |
| 对等连接与 Android 快照 | TCP 8787 | PC 节点之间；Android 与已配对 PC 之间 |
| 桌面 HTTP API/兼容网页 | TCP 8788（默认） | 浏览器或可选 Android WebView 到 PC |

两台 PC 的 `CODEX_MONITOR_NETWORK_ID` 必须一致。未配对节点只会显示在附近列表；显式配对
完成后会保存对端公钥，后续 DHCP 地址变化不需要重新输入 IP 或配对码。

如果 WLAN 阻断 multicast/broadcast，可在每台节点设置
`CODEX_MONITOR_DISCOVERY_TARGETS=devbox-a.local,10.0.0.22:37991,[2001:db8::22]:37991`。
目标以逗号分隔，端口可省略（默认 UDP 发现端口 `37991`）；IPv6 带端口使用 `[addr]:port`。优先
使用能随 DHCP 更新的局域网 DNS/mDNS 主机名，IP 仅作应急。目标只接收定向发现公告，后续仍
是已配对 PC 直连；当前不承担状态权威或 relay，relay 尚未实现，仅作为未来可选扩展；单目标解析/发送失败不会阻止节点启动或其他目标。

## NixOS 主机

推荐导入 flake 的 `nixosModules.codex-monitor`，让服务以能读取本机 Codex 配置的用户运行。
示例和 secret 文件格式见 [`packaging/nixos/README.md`](../packaging/nixos/README.md)。

`environmentFile` 至少可包含：

```text
CODEX_MONITOR_READ_TOKEN=read-secret
CODEX_MONITOR_CONTROL_TOKEN=control-secret
```

配对码是区分大小写的 8 位 ASCII 字母数字串。通过管理界面生成新码后，旧码立即失效；配对成功也会消费该码并自动换代。
未配对节点不会再依赖两端相同的环境变量自动加入网络。通过管理 API 手动配对时输入目标对端当前显示的码；API 不会持久化用户输入的码。
配对码只用于首次建立信任，刷新它不会影响已经保存的对端公钥。

需要让 Poke3 访问时将 `httpBind` 设为 `0.0.0.0:8788`，并把 `openFirewall` 设为 `true`；
只在修改 Nix 配置后由用户手动执行 `nixos-rebuild switch`。

## Windows 10 主机

在 `D:\TemporaryWorkSpace\codex-monitor-UI` 的 PowerShell 中执行：

```powershell
pwsh -File packaging\windows\build.ps1
pwsh -File packaging\windows\firewall.ps1
pwsh -File packaging\windows\run-node.ps1 `
  -NetworkId home-lan `
  -HttpBind 0.0.0.0:8788 `
  -ReadToken read-secret `
  -ControlToken control-secret
```

需要登录后自动启动时，使用 `packaging\windows\install-task.ps1`。它引用用户目录下的
`%LOCALAPPDATA%\CodexMonitor\node.env`，不会把 token 写入任务命令行：

```text
CODEX_MONITOR_NETWORK_ID=home-lan
CODEX_MONITOR_READ_TOKEN=read-secret
CODEX_MONITOR_CONTROL_TOKEN=control-secret
```

创建目录和文件后执行 `pwsh -File packaging\windows\install-task.ps1`；删除登录任务使用
`pwsh -File packaging\windows\install-task.ps1 -Remove`。

多网卡时默认会跳过 Hyper-V、WSL、Mihomo 等虚拟接口；如果仍需指定 WLAN，可增加
`-DiscoveryInterface 10.223.203.176`。若 `codex.cmd` 不在启动进程的 PATH 中，增加
`-CodexBin "C:\path\to\codex.cmd"`。

防火墙脚本只为 Private 网络固定开放 UDP 37991、TCP 8787，并默认开放 HTTP TCP 8788；如自定义
HTTP 端口，需让 `-HttpPort` 与 `CODEX_MONITOR_HTTP_BIND` 保持一致。不要把 HTTP 绑定或端口转发到公网。

## Poke3 / Android

1. 安装 Android APK（Android 10/API 29 或更新版本），确认 Poke3 与至少一个 PC 在同一局域网。
2. 在原生节点页查看本机一次性配对码，并输入 PC 当前显示的码完成首次配对；之后通过持久公钥信任自动重连。
3. Android 使用 UDP 37991 发现节点、TCP 8787 建立 peer 连接；经 P-256 身份认证的快照会本地持久化并显示在原生概览、额度、动态和会话面板。
4. 桌面 TCP 18788 不可达不影响已配对 Android 原生仪表盘查看其本地快照；如果要使用网页兼容查看器，才需要桌面 HTTP 页面和 API 可达。

可选 WebView/PWA 查看器在线时会从 `peers[].httpEndpoint` 和 `peers[].httpEndpoints` 学习其他已配对 PC 的 IP、
主机名和 `.local` 候选。当前入口失联后会自动尝试缓存的其他入口；所有节点使用相同 token
时切换无需重新输入。已打开的页面在网络完全中断时显示最后快照并标为 stale，恢复后自动刷新；关闭后离线重开还需要浏览器已成功缓存页面壳，普通局域网 HTTP 不保证这一点。使用 HTTPS 页面时，节点 API 也需提供 HTTPS，避免混合内容被浏览器拦截。
主机名候选依赖路由器本地 DNS、LLMNR 或 mDNS；这些解析服务不可用时，仍可在“节点地址”中
填写任一台机器的新 IP。

## 可选中央入口

Poke3 APK 本身就是拥有独立身份的 peer，会参与 multicast/broadcast 发现、TCP 配对和可信节点重连；WebView 地址仅供可选兼容查看器使用，不决定 Android 节点身份。
发现受限时，仍可把一台常开机器的局域网主机名或 IP 配到 `CODEX_MONITOR_DISCOVERY_TARGETS`
作为 discovery seed；它不是状态权威或 relay，入口离线不影响其他已配对节点之间收敛。

## 快速检查

在任意能访问节点的终端执行：

```bash
curl http://<pc>:8788/api/v1/health
curl http://<pc>:8788/api/v1/state
```

检查结果中的 `hosts` 是否包含两台 PC，`peers` 是否带有已配对节点和入口候选。控制
接口默认关闭；只有设置 `CODEX_MONITOR_CONTROL_TOKEN` 后，PWA 才会显示活动会话的“中断”按钮。
