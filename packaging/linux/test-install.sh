#!/bin/sh
set -eu

archive=${1:?usage: test-install.sh /path/to/codex-monitor-linux-x86_64.tar.gz}
test_dir=$(mktemp -d)
trap 'rm -rf -- "$test_dir"' EXIT HUP INT TERM
mkdir -p "$test_dir/package" "$test_dir/stage"
tar --no-same-owner -xzf "$archive" -C "$test_dir/package"

DESTDIR="$test_dir/stage" sh "$test_dir/package/scripts/install.sh"
installed_bin="$test_dir/stage$HOME/.local/bin"
test -x "$installed_bin/codex-monitor-node"
test -x "$installed_bin/codex-monitor-agent"
test -f "$test_dir/stage$HOME/.config/systemd/user/codex-monitor.service"

if [ "${RUN_SMOKE:-0}" = 1 ]; then
  set +e
  timeout 3 env \
    CODEX_MONITOR_HTTP_BIND=127.0.0.1:0 \
    CODEX_MONITOR_DISCOVERY_INTERFACE=127.0.0.1 \
    CODEX_MONITOR_CODEX_BIN=/bin/false \
    CODEX_MONITOR_IDENTITY_PATH="$test_dir/identity.json" \
    CODEX_MONITOR_PAIRING_PATH="$test_dir/pairing.json" \
    CODEX_MONITOR_STATE_PATH="$test_dir/state.json" \
    "$installed_bin/codex-monitor-node"
  run_status=$?
  set -e
  test "$run_status" -eq 124
fi

DESTDIR="$test_dir/stage" sh "$test_dir/package/scripts/uninstall.sh"
test ! -e "$installed_bin/codex-monitor-node"
test -d "$test_dir/stage$HOME/.local/share/codex-monitor"
printf '%s\n' DISTROBOX_PACKAGE_TEST_OK
