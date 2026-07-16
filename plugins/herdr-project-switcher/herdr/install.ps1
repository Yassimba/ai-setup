$ErrorActionPreference = "Stop"
# TLS 1.2 for the release downloads on hosts whose .NET default predates it;
# additive (-bor) so stronger protocols stay enabled.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor 3072
# Invoke-WebRequest's progress bar slows downloads badly under PowerShell 5.1.
$script:ProgressPreference = "SilentlyContinue"

$Name = "herdr-project-switcher"
$Repo = "Yassimba/ai-setup"
$Root = Split-Path -Parent $PSScriptRoot
$BinDir = Join-Path $Root "bin"
$versionLine = Get-Content (Join-Path $Root "herdr-plugin.toml") |
  Where-Object { $_ -match '^version' } | Select-Object -First 1
if ($versionLine -notmatch '"([^"]+)"') { throw "$Name`: cannot read version" }
$Tag = "$Name-v$($Matches[1])"

$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
switch ("$arch") {
  "Arm64" { $target = "aarch64-pc-windows-msvc" }
  "X64" { $target = "x86_64-pc-windows-msvc" }
  default { throw "$Name`: unsupported Windows architecture $arch" }
}
$archive = "$Name-$target.zip"
$checksum = "$Name-$target.sha256"
$base = "https://github.com/$Repo/releases/download/$Tag"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  function Get-Asset([string]$Url, [string]$OutFile) {
    for ($attempt = 1; $attempt -le 5; $attempt++) {
      try { Invoke-WebRequest -Uri $Url -OutFile $OutFile -MaximumRedirection 5 -UseBasicParsing; return }
      catch { if ($attempt -eq 5) { throw }; Start-Sleep -Seconds 3 }
    }
  }
  Get-Asset "$base/$archive" (Join-Path $tmp $archive)
  Get-Asset "$base/$checksum" (Join-Path $tmp $checksum)
  $expected = ((Get-Content (Join-Path $tmp $checksum) -Raw).Trim() -split '\s+')[0]
  $actual = (Get-FileHash (Join-Path $tmp $archive) -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($expected.ToLowerInvariant() -ne $actual) { throw "$Name`: checksum mismatch" }
  New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
  Expand-Archive -Path (Join-Path $tmp $archive) -DestinationPath $tmp -Force
  Copy-Item (Join-Path $tmp "$Name.exe") (Join-Path $BinDir "$Name.exe") -Force
  Write-Host "$Name`: installed $(Join-Path $BinDir "$Name.exe")"
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
