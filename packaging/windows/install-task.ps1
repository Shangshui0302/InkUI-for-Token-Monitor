[CmdletBinding(SupportsShouldProcess)]
param(
  [string]$TaskName = "CodexMonitorNode",
  [string]$Binary = (Join-Path $PSScriptRoot "dist\codex-monitor-server.exe"),
  [string]$EnvironmentFile = (Join-Path $env:LOCALAPPDATA "CodexMonitor\node.env"),
  [switch]$Remove
)

$ErrorActionPreference = "Stop"
$task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($Remove) {
  if ($task -and $PSCmdlet.ShouldProcess($TaskName, "remove scheduled task")) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
  }
  exit 0
}

if (-not (Test-Path -LiteralPath $Binary)) {
  throw "Binary not found: $Binary. Run build.ps1 first."
}
if (-not (Test-Path -LiteralPath $EnvironmentFile)) {
  throw "Environment file not found: $EnvironmentFile. Create it before installing the task."
}

$powershell = Get-Command pwsh -ErrorAction SilentlyContinue
if (-not $powershell) { $powershell = Get-Command powershell -ErrorAction SilentlyContinue }
if (-not $powershell) { throw "PowerShell executable not found" }

$runScript = Join-Path $PSScriptRoot "run-node.ps1"
$arguments = "-NoProfile -ExecutionPolicy Bypass -File `"$runScript`" -Binary `"$Binary`" -EnvironmentFile `"$EnvironmentFile`""
$action = New-ScheduledTaskAction -Execute $powershell.Source -Argument $arguments -WorkingDirectory $PSScriptRoot
$trigger = New-ScheduledTaskTrigger -AtLogOn
$principal = New-ScheduledTaskPrincipal -UserId ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited

if ($PSCmdlet.ShouldProcess($TaskName, "register logon task")) {
  Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Principal $principal -Force | Out-Null
  Write-Host "Codex Monitor logon task installed: $TaskName"
  Write-Host "Environment file: $EnvironmentFile"
}
