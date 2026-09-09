# Windows packaging

计划中的交付：

- `codex-monitor-server.exe`；
- `codex-monitor-agent.exe`；
- Private Network 防火墙规则脚本；
- 首版使用计划任务/登录启动，稳定后再评估 Windows Service。

Monitor 不保存 Codex 凭据，Agent 只连接本机的 Codex App Server。
