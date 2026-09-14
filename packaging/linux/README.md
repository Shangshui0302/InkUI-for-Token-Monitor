# Portable Linux x86_64 package

Build locally with `nix build .#codex-monitor-linux-x86_64`. The result contains
`codex-monitor-linux-x86_64.tar.gz`, a native x86_64 Linux archive with statically
linked musl binaries, a systemd user unit, and unprivileged install/uninstall scripts.
It does not contain Nix-store runtime paths or credentials. The archive is for local
installation/testing; this repository does not publish or deploy it.

Extract the archive and run `scripts/install.sh`. This installs binaries under
`~/.local/bin` and the unit under
`$XDG_CONFIG_HOME/systemd/user`. To enable it, run
`systemctl --user enable --now codex-monitor.service`. The local HTTP interface
defaults to `127.0.0.1:8788`.

Optional variables can be placed in
`$XDG_CONFIG_HOME/codex-monitor/codex-monitor.env`; the service reads this file
without embedding its contents in the package. For example:

```sh
install -d -m 700 "$HOME/.config/codex-monitor"
umask 077
cat > "$HOME/.config/codex-monitor/codex-monitor.env" <<'EOF'
CODEX_MONITOR_PAIR_CODE=Ab3dE7qK
CODEX_MONITOR_READ_TOKEN=change-me-too
CODEX_MONITOR_CODEX_BIN=/absolute/path/to/codex
EOF
```

Keep real secrets only in that user-owned file; never pass them as Nix options or
build arguments. The uninstall script disables/removes the unit and binaries but
preserves user configuration and node state. No root access is required.

Pairing codes are case-sensitive 8-character ASCII alphanumeric strings. For
automatic first pairing, configure the same code on both nodes; a manual pairing
request uses the target node's code, so the two local codes need not match. Replace
legacy six-digit `CODEX_MONITOR_PAIR_CODE` values. Changing a code does not affect
already-paired public keys.

Packagers may set `DESTDIR` to stage or test installation without invoking
`systemctl`; normal users should leave it unset.
