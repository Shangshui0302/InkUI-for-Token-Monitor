# Windows packaging

当前交付：

- `build.ps1` 生成 `codex-monitor-server.exe` 和 `codex-monitor-agent.exe`；
- `firewall.ps1` 幂等配置 Private 网络的固定 TCP `8787`、UDP `37991`，以及默认 TCP `8788` 的 HTTP 端口（可用 `-HttpPort` 覆盖）；
- `run-node.ps1` 用参数或独立 env 文件启动节点，避免每次手动拼接配置；
- `install-task.ps1` 为当前用户安装/删除登录时自动启动的 Scheduled Task。

如果源码放在 `D:\TemporaryWorkSpace\codex-monitor-UI`，从 PowerShell 进入该目录后运行下面的命令；
脚本不会把构建产物写入 C 盘临时目录。

在仓库根目录运行：

```powershell
pwsh -File packaging\windows\build.ps1
pwsh -File packaging\windows\firewall.ps1
pwsh -File packaging\windows\run-node.ps1 -ReadToken "read-secret" -ControlToken "control-secret"
```

长期运行时把 token 放在用户目录的 env 文件，避免写进计划任务命令行：

```text
# %LOCALAPPDATA%\CodexMonitor\node.env
CODEX_MONITOR_NETWORK_ID=home-lan
CODEX_MONITOR_READ_TOKEN=read-secret
CODEX_MONITOR_CONTROL_TOKEN=control-secret
```

创建文件后安装登录启动任务：

```powershell
New-Item -ItemType Directory -Force "$env:LOCALAPPDATA\CodexMonitor"
pwsh -File packaging\windows\install-task.ps1
```

删除任务：`pwsh -File packaging\windows\install-task.ps1 -Remove`。

多网卡主机默认会跳过 Hyper-V、WSL、Mihomo 等虚拟接口；如果仍需固定发现接口，可传
`-DiscoveryInterface 10.223.203.176`（也支持接口名）。

如果 `codex.cmd` 不在服务进程的 PATH 中，可用 `-CodexBin "C:\path\to\codex.cmd"` 指定。

防火墙脚本需要管理员 PowerShell；先确认 WLAN 配置为 `Private`。脚本只开放本地网段所需
端口，不改变 Codex CLI 或凭据目录。

配对码是区分大小写的 8 位 ASCII 字母数字串。未配对节点只会被发现；在管理界面生成新码，
并在发起端输入目标对端当前的码。刷新或一次成功配对会令旧码失效；已保存的对端公钥不受影响。

Monitor 不保存 Codex 凭据，Agent 只连接本机的 Codex App Server。

M2/M3.1 的只读 HTTP API 和 Poke3 页面默认绑定 `127.0.0.1:8788`，仅用于本机验证；要让
Poke3 访问，可设置 `CODEX_MONITOR_HTTP_BIND=0.0.0.0:8788`，并用 Windows 防火墙限制为
Private 网络。页面默认轮询 `/api/v1/state`，也保留 `/api/state` 兼容路径；设置
`CODEX_MONITOR_READ_TOKEN` 后 API 需要 `Authorization: Bearer <token>`，PWA 的节点设置面板
可填写地址和 token。未设置 token 时仍是只读但不认证，不要暴露到公网。Windows 上若 Codex
CLI 由 Scoop/npm 提供，collector 会调用 `codex.cmd`；也可用
`CODEX_MONITOR_CODEX_BIN` 指定可执行文件。

如需控制会话，另设置 `CODEX_MONITOR_CONTROL_TOKEN`；这只启用经过配对/签名 peer 路由的
`POST /api/v1/control` interrupt，且必须带 `threadId`、`turnId`、`requestId` 与短期 `expiresAt`。
控制 token 与只读 token 分离，未设置时控制入口关闭。

页面使用按钮切换总览、额度、活动和会话；额度显示“五小时限制”“一周限制”，活动页显示
App Server 返回的最近 14 天 `dailyUsageBuckets` 热力图。PWA 节点设置面板可分别保存只读 token
和控制 token；拿到活动 `turnId` 时，会话页显示受限“中断”按钮。
