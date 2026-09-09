# NixOS packaging

计划中的交付：

- `codex-monitor-agent` Nix package；
- 可选 `codex-monitor` systemd service module；
- 明确区分 package/build、activation 和 runtime 验证。

实现阶段再根据锁定的 nixpkgs 接口添加 derivation，不在初始化阶段猜测运行时
依赖或自动执行 `nixos-rebuild switch`。
