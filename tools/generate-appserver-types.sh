#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
exec codex app-server generate-ts --experimental \
  --out "$repo_root/generated/codex-app-server"
