[CmdletBinding(SupportsShouldProcess)]
param(
  [switch]$Remove,
  [int]$HttpPort = 8788
)

$ErrorActionPreference = "Stop"
# The current binary fixes the peer and discovery ports; only HTTP is configurable.
$PeerTcpPort = 8787
$DiscoveryUdpPort = 37991
$rules = @(
  @{ Name = "Codex Monitor mesh TCP"; Protocol = "TCP"; Port = $PeerTcpPort },
  @{ Name = "Codex Monitor HTTP PWA"; Protocol = "TCP"; Port = $HttpPort },
  @{ Name = "Codex Monitor discovery UDP"; Protocol = "UDP"; Port = $DiscoveryUdpPort }
)

foreach ($rule in $rules) {
  if ($Remove) {
    if ($PSCmdlet.ShouldProcess($rule.Name, "remove firewall rule")) {
      Remove-NetFirewallRule -DisplayName $rule.Name -ErrorAction SilentlyContinue
    }
    continue
  }

  if ($PSCmdlet.ShouldProcess($rule.Name, "allow on Private networks")) {
    Remove-NetFirewallRule -DisplayName $rule.Name -ErrorAction SilentlyContinue
    New-NetFirewallRule `
      -DisplayName $rule.Name `
      -Direction Inbound `
      -Action Allow `
      -Profile Private `
      -Protocol $rule.Protocol `
      -LocalPort $rule.Port | Out-Null
  }
}

Write-Host "Codex Monitor Private-network rules updated. Run PowerShell as Administrator."
