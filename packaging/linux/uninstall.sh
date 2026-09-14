#!/bin/sh
set -eu

if [ -z "${HOME:-}" ]; then
    printf '%s\n' 'HOME must be set.' >&2
    exit 1
fi
prefix=$HOME/.local
install_root=${DESTDIR:-}
config_home=${XDG_CONFIG_HOME:-"$HOME/.config"}
unit_dir=$install_root$config_home/systemd/user

if [ -z "$install_root" ] && command -v systemctl >/dev/null 2>&1 && [ -n "${XDG_RUNTIME_DIR:-}" ]; then
    systemctl --user disable --now codex-monitor.service 2>/dev/null || :
fi
rm -f "$unit_dir/codex-monitor.service" \
    "$install_root$prefix/bin/codex-monitor-node" "$install_root$prefix/bin/codex-monitor-agent"
if [ -z "$install_root" ] && command -v systemctl >/dev/null 2>&1 && [ -n "${XDG_RUNTIME_DIR:-}" ]; then
    systemctl --user daemon-reload
fi
printf '%s\n' 'Removed the installed service and binaries. User configuration and node data were preserved.'
