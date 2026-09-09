# Windows packaging

计划中的交付：

- `codex-monitor-server.exe`；
- `codex-monitor-agent.exe`；
- Private Network 防火墙规则脚本；
- 首版使用计划任务/登录启动，稳定后再评估 Windows Service。

Monitor 不保存 Codex 凭据，Agent 只连接本机的 Codex App Server。

M2/M3.1 的只读 HTTP API 和 Poke3 页面默认绑定 `127.0.0.1:8788`，仅用于本机验证；要让
Poke3 访问，可设置 `CODEX_MONITOR_HTTP_BIND=0.0.0.0:8788`，并用 Windows 防火墙限制为
Private 网络。页面轮询 `/api/state`，接口仍未认证，不要暴露到公网。Windows 上若 Codex
CLI 由 Scoop/npm 提供，collector 会调用 `codex.cmd`；也可用
`CODEX_MONITOR_CODEX_BIN` 指定可执行文件。

页面使用按钮切换总览、额度、活动和会话；额度显示“五小时限制”“一周限制”，活动页显示
App Server 返回的最近 14 天 `dailyUsageBuckets` 热力图。
