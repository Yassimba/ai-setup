$ErrorActionPreference = "Stop"
# TLS 1.2 for the release downloads on hosts whose .NET default predates it;
# additive (-bor) so stronger protocols stay enabled.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor 3072
# Invoke-WebRequest's progress bar slows downloads badly under PowerShell 5.1.
$script:ProgressPreference = "SilentlyContinue"

$Name = "ai-setup"
$Version = "0.6.1"
$Repo = "Yassimba/ai-setup"
$Tag = "ai-setup-v$Version"
$InstallDir = if ($env:AI_SETUP_INSTALL_DIR) {
  $env:AI_SETUP_INSTALL_DIR
} elseif ($env:YASSIMBA_INSTALL_DIR) {
  $env:YASSIMBA_INSTALL_DIR
} else {
  Join-Path $HOME ".local\bin"
}

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
      try {
        Invoke-WebRequest -Uri $Url -OutFile $OutFile -MaximumRedirection 5 -UseBasicParsing
        return
      } catch {
        if ($attempt -eq 5) { throw }
        Start-Sleep -Seconds 3
      }
    }
  }

  Get-Asset "$base/$archive" (Join-Path $tmp $archive)
  Get-Asset "$base/$checksum" (Join-Path $tmp $checksum)
  $expected = ((Get-Content (Join-Path $tmp $checksum) -Raw).Trim() -split '\s+')[0]
  $actual = (Get-FileHash (Join-Path $tmp $archive) -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($expected.ToLowerInvariant() -ne $actual) { throw "$Name`: checksum mismatch" }

  New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
  Expand-Archive -Path (Join-Path $tmp $archive) -DestinationPath $tmp -Force
  $exe = Join-Path $InstallDir "$Name.exe"
  # Windows locks a running executable against writes but allows renames, so
  # self-update moves the old binary aside instead of overwriting it. Stale
  # renamed copies from earlier updates are swept first, best effort.
  Get-ChildItem -Path $InstallDir -Filter "$Name.exe.*.old" -ErrorAction SilentlyContinue |
    Remove-Item -Force -ErrorAction SilentlyContinue
  if (Test-Path $exe) {
    Move-Item $exe "$exe.$([System.Guid]::NewGuid().ToString('N')).old" -Force
  }
  Copy-Item (Join-Path $tmp "$Name.exe") $exe
  Remove-Item (Join-Path $InstallDir "yassimba.exe") -Force -ErrorAction SilentlyContinue

  # Edit the raw registry value: [Environment]::GetEnvironmentVariable expands
  # entries like %USERPROFILE%\bin, and writing the expanded result back would
  # hardcode them permanently.
  $envKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Environment", $true)
  if ($null -eq $envKey) {
    $envKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Environment")
  }
  try {
    $userPath = [string]$envKey.GetValue("Path", "", [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $pathEntries = @($userPath -split ';' | Where-Object { $_ })
    $expanded = @($pathEntries | ForEach-Object { [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') })
    if ($expanded -notcontains $InstallDir.TrimEnd('\')) {
      $envKey.SetValue("Path", (($pathEntries + $InstallDir) -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
      # Broadcast WM_SETTINGCHANGE so Explorer and already-open hosts reload
      # the environment; without it the new PATH waits for a re-login. Best
      # effort: a locked-down host may refuse the P/Invoke compile.
      try {
        if (-not ("AiSetup.NativeMethods" -as [type])) {
          $signature = '[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)] public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);'
          Add-Type -Namespace AiSetup -Name NativeMethods -MemberDefinition $signature
        }
        $broadcastResult = [UIntPtr]::Zero
        # HWND_BROADCAST (0xffff), WM_SETTINGCHANGE (0x1a), SMTO_ABORTIFHUNG (0x2).
        [void][AiSetup.NativeMethods]::SendMessageTimeout([IntPtr]0xffff, 0x1a, [UIntPtr]::Zero, "Environment", 0x2, 5000, [ref]$broadcastResult)
      } catch {
        # The PATH is persisted either way; new logins always see it.
      }
      Write-Host "Added $InstallDir to your user PATH. Most new terminals pick it up immediately; if ai-setup is not found, open a new terminal."
    }
  } finally {
    $envKey.Close()
  }
  # Make the binary reachable in this session too (the README one-liner runs a
  # nested powershell, which inherits the pre-install environment).
  $sessionEntries = @($env:Path -split ';' | Where-Object { $_ } | ForEach-Object { $_.TrimEnd('\') })
  if ($sessionEntries -notcontains $InstallDir.TrimEnd('\')) {
    $env:Path = "$env:Path".TrimEnd(';') + ";" + $InstallDir
  }
  Write-Host "$Name $Version installed to $(Join-Path $InstallDir "$Name.exe")"
  Write-Host "Next: ai-setup setup"
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
