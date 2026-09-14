[CmdletBinding()]
param(
  [string]$OutputDir = (Join-Path $PSScriptRoot "dist")
)

$ErrorActionPreference = "Stop"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\.."))

Push-Location $repo
try {
  cargo build --locked --release --bin codex-monitor-server --bin codex-monitor-agent

  New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
  Copy-Item "target\release\codex-monitor-server.exe" (Join-Path $OutputDir "codex-monitor-server.exe") -Force
  Copy-Item "target\release\codex-monitor-agent.exe" (Join-Path $OutputDir "codex-monitor-agent.exe") -Force
  Write-Host "Binaries written to $OutputDir"
}
finally {
  Pop-Location
}
