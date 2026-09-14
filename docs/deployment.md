# Codex Monitor 部署

这套部署不需要固定 IP，也不需要 Poke3 运行 SSH。两台 PC 都运行同一个节点程序；任意一台
都可以作为 Poke3 的入口，节点会通过 UDP 发现和已配对 TCP 连接同步状态。

## 端口和配置

| 用途 | 默认端口 | 方向 |
| --- | ---: | --- |
| 节点发现 | UDP 37991 | 局域网广播/multicast |
| PC 对等连接 | TCP 8787 | PC 节点之间 |
| Poke3/API | TCP 8788 | Poke3 到 PC |

两台 PC 的 `CODEX_MONITOR_NETWORK_ID` 和首次配对用的
`CODEX_MONITOR_PAIR_CODE` 必须一致。配对完成后会保存对端公钥，后续 DHCP 地址变化不需要
重新输入 IP 或配对码。

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
CODEX_MONITOR_PAIR_CODE=Ab3dE7qK
CODEX_MONITOR_READ_TOKEN=read-secret
CODEX_MONITOR_CONTROL_TOKEN=control-secret
```

服务启动日志会打印当前 `pairing code:`。配对码是区分大小写的 8 位 ASCII 字母数字串；自动初次连接时两端要设置同一个
`CODEX_MONITOR_PAIR_CODE`。通过管理 API 手动配对时输入目标对端的码，不要求两端本地码相同；API 不会持久化用户输入的配对码。
旧版本的 6 位配置必须换成 8 位。配对码只用于初次配对，变更它不会影响已经保存的对端公钥。

需要让 Poke3 访问时将 `httpBind` 设为 `0.0.0.0:8788`，并把 `openFirewall` 设为 `true`；
只在修改 Nix 配置后由用户手动执行 `nixos-rebuild switch`。

## Windows 10 主机

在 `D:\TemporaryWorkSpace\codex-monitor-UI` 的 PowerShell 中执行：

```powershell
pwsh -File packaging\windows\build.ps1
pwsh -File packaging\windows\firewall.ps1
pwsh -File packaging\windows\run-node.ps1 `
  -NetworkId home-lan `
  -PairCode Ab3dE7qK `
  -HttpBind 0.0.0.0:8788 `
  -ReadToken read-secret `
  -ControlToken control-secret
```

需要登录后自动启动时，使用 `packaging\windows\install-task.ps1`。它引用用户目录下的
`%LOCALAPPDATA%\CodexMonitor\node.env`，不会把 token 写入任务命令行：

```text
CODEX_MONITOR_NETWORK_ID=home-lan
CODEX_MONITOR_PAIR_CODE=Ab3dE7qK
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

1. 确认 Poke3 与任一 PC 在同一局域网。
2. 浏览器打开 `http://<任一PC的当前IP>:8788/`。
3. 在“节点地址”中保存入口和只读 token；需要中断会话时再填写独立控制 token。
4. 局域网 HTTP 可在线使用网页或 Android APK；浏览器可能只提供主屏幕快捷方式。完整 PWA 安装与离线页面缓存需要 HTTPS（开发用 localhost 是例外）。

PWA 在线时会从 `peers[].httpEndpoint` 和 `peers[].httpEndpoints` 学习其他已配对 PC 的 IP、
主机名和 `.local` 候选。当前入口失联后会自动尝试缓存的其他入口；所有节点使用相同 token
时切换无需重新输入。已打开的页面在网络完全中断时显示最后快照并标为 stale，恢复后自动刷新；关闭后离线重开还需要浏览器已成功缓存页面壳，普通局域网 HTTP 不保证这一点。使用 HTTPS 页面时，节点 API 也需提供 HTTPS，避免混合内容被浏览器拦截。
主机名候选依赖路由器本地 DNS、LLMNR 或 mDNS；这些解析服务不可用时，仍可在“节点地址”中
填写任一台机器的新 IP。

## 可选中央入口

不需要额外购买设备。把任意一台常开的 PC 地址手工填入 Poke3 即可作为入口；multicast/broadcast
受限时，也可把它的局域网主机名或 IP 配到 `CODEX_MONITOR_DISCOVERY_TARGETS` 作为 discovery
seed。它只是入口、缓存或发现 seed，业务状态仍由各 PC 对等同步；当前不是状态权威或状态 relay，
relay 尚未实现，仅作为未来可选扩展，入口离线不影响已配对 PC 之间收敛。

## 快速检查

在任意能访问节点的终端执行：

```bash
curl http://<pc>:8788/api/v1/health
curl http://<pc>:8788/api/v1/state
```

检查结果中的 `hosts` 是否包含两台 PC，`peers` 是否带有已配对节点和入口候选。控制
接口默认关闭；只有设置 `CODEX_MONITOR_CONTROL_TOKEN` 后，PWA 才会显示活动会话的“中断”按钮。
