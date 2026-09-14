# NixOS packaging

当前交付：

- `nix build .#codex-monitor-node` 生成节点和兼容 Agent；
- `nixosModules.codex-monitor` 提供可选 systemd 服务；
- 明确区分 package/build、activation 和 runtime 验证。

在宿主机的 flake 中导入模块，并让服务以能读取本机 Codex 配置的用户运行：

```nix
{
  inputs.codex-monitor.url = "path:/path/to/codex-monitor-UI";

  outputs = { self, nixpkgs, codex-monitor, ... }: {
    nixosConfigurations.host = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        codex-monitor.nixosModules.codex-monitor
        ({ ... }: {
          services.codex-monitor = {
            enable = true;
            user = "lishangshui";
            group = "users";
            networkId = "home-lan";
            httpBind = "0.0.0.0:8788";
            codexBin = "/etc/profiles/per-user/lishangshui/bin/codex";
            openFirewall = true;
            # Secret values stay outside the Nix store.
            environmentFile = "/run/secrets/codex-monitor.env";
          };
        })
      ];
    };
  };
}
```

`/run/secrets/codex-monitor.env` 可包含 `CODEX_MONITOR_PAIR_CODE`、
`CODEX_MONITOR_READ_TOKEN` 和 `CODEX_MONITOR_CONTROL_TOKEN`。服务用户必须能运行 `codex`
并读取其本地 Codex 配置；`nixos-rebuild switch` 仍由用户手动执行。

配对码是区分大小写的 8 位 ASCII 字母数字串。自动初次连接时，两端需配置相同的码；管理 API
手动配对则填写目标对端的码，两端本地码可以不同。原先的 6 位环境变量必须更换，已保存的
对端公钥不会因此改变。

节点需要接收对等连接和 LAN 发现公告；启用 NixOS 防火墙时，模块会按 `openFirewall = true`
放行 TCP `8787`、TCP `8788` 和 UDP `37991`。只有迁移期的旧 Agent 只需对 Monitor 出站连接。
