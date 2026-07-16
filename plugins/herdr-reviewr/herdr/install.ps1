# herdr `[[build]]` step (Windows): download the prebuilt herdr-reviewr.exe for this platform
# from the matching GitHub Release into the plugin's bin/ dir. Runs on `herdr plugin install`;
# `herdr plugin link` skips the build step; for a local checkout, build from source with
# `cargo install --path .`.
#
# The build runs with the plugin checkout as the working directory, so the plugin root is
# resolved from this script's location rather than $env:HERDR_PLUGIN_ROOT (build commands may
# not receive the runtime env). At runtime the pane command reads
# %HERDR_PLUGIN_ROOT%\bin\herdr-reviewr.exe.
$ErrorActionPreference = "Stop"
# TLS 1.2 for the release downloads on hosts whose .NET default predates it;
# additive (-bor) so stronger protocols stay enabled.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor 3072
# Invoke-WebRequest's progress bar slows downloads badly under PowerShell 5.1.
$script:ProgressPreference = "SilentlyContinue"

$Name = "herdr-reviewr"
$Repo = "Yassimba/ai-setup"

$Root = Split-Path -Parent $PSScriptRoot
$BinDir = Join-Path $Root "bin"

# The release tag is per-plugin in the skills monorepo (herdr-reviewr-v<version>) and matches
# the manifest version, so a checkout always pulls its own release.
$manifest = Get-Content (Join-Path $Root "herdr-plugin.toml")
$versionLine = $manifest | Where-Object { $_ -match '^version' } | Select-Object -First 1
if ($versionLine -notmatch '"([^"]+)"') { throw "$($Name): cannot read version from herdr-plugin.toml" }
$Version = $Matches[1]
$Tag = "$Name-v$Version"

# Map the running platform to the release target triple.
$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
switch ("$arch") {
  "Arm64" { $target = "aarch64-pc-windows-msvc" }
  "X64"   { $target = "x86_64-pc-windows-msvc" }
  default { throw "$($Name): no prebuilt binary for Windows-$arch; build from source with 'cargo install --path .'" }
}

$archive = "$Name-$target.zip"
# taiki-e's checksum sidecar drops the archive extension: <name>-<target>.sha256, not <archive>.sha256.
$checksum = "$Name-$target.sha256"
$base = "https://github.com/$Repo/releases/download/$Tag"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  # Release-asset downloads are eventually-consistent: GitHub's CDN can 404 for a few minutes
  # after a release publishes. Retry so an install right after a release doesn't fail spuriously.
  function Get-Asset([string]$Url, [string]$OutFile) {
    $delaySeconds = 3
    for ($attempt = 1; $attempt -le 5; $attempt++) {
      try {
        Invoke-WebRequest -Uri $Url -OutFile $OutFile -MaximumRedirection 5 -UseBasicParsing
        return
      } catch {
        if ($attempt -eq 5) { throw }
        Start-Sleep -Seconds $delaySeconds
      }
    }
  }

  Write-Host "$($Name): downloading $archive ($Tag)"
  Get-Asset "$base/$archive" (Join-Path $tmp $archive)
  Get-Asset "$base/$checksum" (Join-Path $tmp $checksum)

  Write-Host "$($Name): verifying checksum"
  $expected = ((Get-Content (Join-Path $tmp $checksum) -Raw).Trim() -split '\s+')[0]
  $actual = (Get-FileHash (Join-Path $tmp $archive) -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($expected.ToLowerInvariant() -ne $actual) {
    throw "$($Name): checksum mismatch (expected $expected, got $actual)"
  }

  New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
  Expand-Archive -Path (Join-Path $tmp $archive) -DestinationPath $tmp -Force
  Copy-Item (Join-Path $tmp "$Name.exe") (Join-Path $BinDir "$Name.exe") -Force
  Write-Host "$($Name): installed $(Join-Path $BinDir "$Name.exe")"
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
