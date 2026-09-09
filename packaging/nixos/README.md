# NixOS packaging

计划中的交付：

- `codex-monitor-agent` Nix package；
- 可选 `codex-monitor` systemd service module；
- 明确区分 package/build、activation 和 runtime 验证。

实现阶段再根据锁定的 nixpkgs 接口添加 derivation，不在初始化阶段猜测运行时
依赖或自动执行 `nixos-rebuild switch`。

Agent 需要接收 Monitor 的 LAN 发现公告；启用 NixOS 防火墙时，模块应声明放行 UDP
`37991`（TCP `8787` 为 Agent 的出站连接，不需要入站端口）。
