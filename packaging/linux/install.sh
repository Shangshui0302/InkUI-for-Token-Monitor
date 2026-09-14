#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [ -z "${HOME:-}" ]; then
    printf '%s\n' 'HOME must be set.' >&2
    exit 1
fi
prefix=$HOME/.local
install_root=${DESTDIR:-}
bin_dir=$install_root$prefix/bin
unit_dir=$install_root${XDG_CONFIG_HOME:-"$HOME/.config"}/systemd/user
data_dir=$install_root${XDG_DATA_HOME:-"$HOME/.local/share"}/codex-monitor

mkdir -p "$bin_dir" "$unit_dir" "$data_dir"
install -m 755 "$script_dir/../bin/codex-monitor-node" "$bin_dir/codex-monitor-node"
install -m 755 "$script_dir/../bin/codex-monitor-agent" "$bin_dir/codex-monitor-agent"
install -m 644 "$script_dir/../systemd/user/codex-monitor.service" "$unit_dir/codex-monitor.service"

if [ -z "$install_root" ] && command -v systemctl >/dev/null 2>&1 && [ -n "${XDG_RUNTIME_DIR:-}" ]; then
    systemctl --user daemon-reload
    printf '%s\n' 'Installed. Start with: systemctl --user enable --now codex-monitor.service'
else
    printf '%s\n' 'Installed files. A user systemd session is not active; start it when available.'
fi
printf 'Optional secrets/config: %s%s/codex-monitor/codex-monitor.env\n' "$install_root" "${XDG_CONFIG_HOME:-$HOME/.config}"
