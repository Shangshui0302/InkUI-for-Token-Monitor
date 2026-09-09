# Windows packaging

计划中的交付：

- `codex-monitor-server.exe`；
- `codex-monitor-agent.exe`；
- Private Network 防火墙规则脚本；
- 首版使用计划任务/登录启动，稳定后再评估 Windows Service。

Monitor 不保存 Codex 凭据，Agent 只连接本机的 Codex App Server。

M2 的只读 HTTP API 默认绑定 `127.0.0.1:8788`，仅用于本机验证；后续 Poke3 接入前再
通过明确的 LAN 绑定、防火墙规则和认证开放访问。Windows 上若 Codex CLI 由 Scoop/npm
提供，collector 会调用 `codex.cmd`；也可用 `CODEX_MONITOR_CODEX_BIN` 指定可执行文件。
