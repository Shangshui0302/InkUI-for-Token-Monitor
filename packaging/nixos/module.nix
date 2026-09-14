{ config, lib, pkgs, defaultPackage }:

with lib;

let
  cfg = config.services.codex-monitor;
in {
  options.services.codex-monitor = {
    enable = mkEnableOption "Codex Monitor peer node";

    package = mkOption {
      type = types.package;
      default = defaultPackage;
      description = "Package providing codex-monitor-node.";
    };

    user = mkOption {
      type = types.str;
      default = "codex-monitor";
      description = "Unix user that owns the node process and its Codex access.";
    };

    group = mkOption {
      type = types.str;
      default = "users";
      description = "Unix group for the node process.";
    };

    networkId = mkOption {
      type = types.str;
      default = "codex-monitor-lan";
      description = "Logical LAN mesh identifier.";
    };

    httpBind = mkOption {
      type = types.str;
      default = "127.0.0.1:8788";
      description = "HTTP bind address for the PWA and JSON API.";
    };

    codexBin = mkOption {
      type = types.str;
      default = "codex";
      description = "Codex executable or absolute path visible to systemd.";
    };

    environmentFile = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = ''
        Optional systemd EnvironmentFile containing CODEX_MONITOR_READ_TOKEN
        and/or CODEX_MONITOR_CONTROL_TOKEN.
      '';
    };

    discoveryInterface = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Optional LAN IPv4 address or interface name for discovery.";
    };

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = "Open mesh, HTTP and discovery ports in the NixOS firewall.";
    };
  };

  config = mkIf cfg.enable {
    systemd.services.codex-monitor = {
      description = "Codex Monitor peer node";
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];
      serviceConfig = {
        ExecStart = "${cfg.package}/bin/codex-monitor-node";
        Restart = "on-failure";
        RestartSec = 5;
        User = cfg.user;
        Group = cfg.group;
        WorkingDirectory = "/var/lib/codex-monitor";
        StateDirectory = "codex-monitor";
        Environment = [
          "CODEX_MONITOR_NETWORK_ID=${cfg.networkId}"
          "CODEX_MONITOR_HTTP_BIND=${cfg.httpBind}"
          "CODEX_MONITOR_CODEX_BIN=${cfg.codexBin}"
        ] ++ optional (cfg.discoveryInterface != null)
          "CODEX_MONITOR_DISCOVERY_INTERFACE=${cfg.discoveryInterface}";
        EnvironmentFile = optional (cfg.environmentFile != null) cfg.environmentFile;
      };
    };

    users.groups.${cfg.group} = { };
    users.users.codex-monitor = mkIf (cfg.user == "codex-monitor") {
      isSystemUser = true;
      group = cfg.group;
      home = "/var/lib/codex-monitor";
      createHome = true;
    };

    networking.firewall.allowedTCPPorts = mkIf cfg.openFirewall [ 8787 8788 ];
    networking.firewall.allowedUDPPorts = mkIf cfg.openFirewall [ 37991 ];
  };
}
