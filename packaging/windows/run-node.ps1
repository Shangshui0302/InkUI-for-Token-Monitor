[CmdletBinding()]
param(
  [string]$Binary = (Join-Path $PSScriptRoot "dist\codex-monitor-server.exe"),
  [string]$NetworkId = "codex-monitor-lan",
  [string]$PairCode,
  [string]$HttpBind = "0.0.0.0:8788",
  [string]$DiscoveryInterface,
  [string]$CodexBin,
  [string]$ReadToken,
  [string]$ControlToken,
  [string]$EnvironmentFile
)

$ErrorActionPreference = "Stop"
if (-not (Test-Path -LiteralPath $Binary)) {
  throw "Binary not found: $Binary. Run build.ps1 first."
}

$environmentValues = @{}
if ($EnvironmentFile) {
  if (-not (Test-Path -LiteralPath $EnvironmentFile)) {
    throw "Environment file not found: $EnvironmentFile"
  }
  foreach ($line in Get-Content -LiteralPath $EnvironmentFile) {
    $trimmed = $line.Trim().TrimStart([char]0xFEFF)
    if (-not $trimmed -or $trimmed.StartsWith("#")) { continue }
    $parts = $trimmed -split "=", 2
    if ($parts.Count -ne 2) { throw "Invalid environment line in $EnvironmentFile" }
    $name = $parts[0].Trim()
    $value = $parts[1].Trim()
    if ($value.Length -ge 2 -and (($value.StartsWith('"') -and $value.EndsWith('"')) -or ($value.StartsWith("'") -and $value.EndsWith("'")))) {
      $value = $value.Substring(1, $value.Length - 2)
    }
    if ($name -notmatch '^CODEX_MONITOR_[A-Z0-9_]+$') {
      throw "Invalid environment variable name '$name' in $EnvironmentFile"
    }
    $environmentValues[$name] = $value
  }
  foreach ($entry in $environmentValues.GetEnumerator()) {
    [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, "Process")
  }
}

$bound = $PSBoundParameters
$networkIdValue = if ($bound.ContainsKey("NetworkId")) { $NetworkId } elseif ($environmentValues.ContainsKey("CODEX_MONITOR_NETWORK_ID")) { $environmentValues["CODEX_MONITOR_NETWORK_ID"] } else { $NetworkId }
$httpBindValue = if ($bound.ContainsKey("HttpBind")) { $HttpBind } elseif ($environmentValues.ContainsKey("CODEX_MONITOR_HTTP_BIND")) { $environmentValues["CODEX_MONITOR_HTTP_BIND"] } else { $HttpBind }
$env:CODEX_MONITOR_NETWORK_ID = $networkIdValue
$env:CODEX_MONITOR_HTTP_BIND = $httpBindValue

$optionalParameters = @{
  DiscoveryInterface = "CODEX_MONITOR_DISCOVERY_INTERFACE"
  PairCode = "CODEX_MONITOR_PAIR_CODE"
  CodexBin = "CODEX_MONITOR_CODEX_BIN"
  ReadToken = "CODEX_MONITOR_READ_TOKEN"
  ControlToken = "CODEX_MONITOR_CONTROL_TOKEN"
}
foreach ($parameter in $optionalParameters.Keys) {
  $name = $optionalParameters[$parameter]
  if ($bound.ContainsKey($parameter)) {
    $value = Get-Variable -Name $parameter -ValueOnly
    [Environment]::SetEnvironmentVariable($name, $value, "Process")
  } elseif ($environmentValues.ContainsKey($name)) {
    [Environment]::SetEnvironmentVariable($name, $environmentValues[$name], "Process")
  }
}

& $Binary
exit $LASTEXITCODE
