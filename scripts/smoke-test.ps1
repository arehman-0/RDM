<#
.SYNOPSIS
  Install → run → uninstall smoke test for the NSIS installer (Windows).

.DESCRIPTION
  1. Silent install (/S) into a temp folder.
  2. Launch the app with a minimal PATH (so tools on your PATH don't count),
     wait for the browser bridge (GET /v1/ping) and for yt-dlp, QuickJS and
     FFmpeg to be in the components folder (LITE downloads them; FULL moves
     its bundled copies there). Pinned SHA-256s are checked.
  3. Turn on "Start with Windows" the way the app does (HKCU Run value).
  4. Quit through the tray-action test hook, silent uninstall with
     /DELETEAPPDATA, and check that nothing is left: install folder,
     shortcuts, uninstall/registry keys, the Run value, app data.

  Your own IDM Replica data (%APPDATA% / %LOCALAPPDATA%\com.idmreplica.app)
  is moved aside first and put back at the end, even if a step fails.
  Refuses to run if IDM Replica is running or already installed.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\smoke-test.ps1 -Installer release\IDM-Replica-0.1.0-x64-lite-setup.exe
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory)][string]$Installer,
  [int]$TimeoutSec = 600
)

$ErrorActionPreference = 'Stop'
$Installer = (Resolve-Path $Installer).Path
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$product = 'IDM Replica'
$bundleId = 'com.idmreplica.app'
$publisher = 'Abdul Rehman'
$uninstKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$product"
$manuKey = "HKCU:\Software\$publisher\$product"
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$roaming = Join-Path $env:APPDATA $bundleId
$local = Join-Path $env:LOCALAPPDATA $bundleId
$startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\$product"
$desktopLnk = Join-Path ([Environment]::GetFolderPath('Desktop')) "$product.lnk"
$dir = Join-Path $env:TEMP ("idm-smoke-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))
$exe = Join-Path $dir "$product.exe"
$results = New-Object System.Collections.Generic.List[object]

function Check([string]$Name, [bool]$Ok, [string]$Detail = '') {
  $results.Add([pscustomobject]@{ Result = $(if ($Ok) { 'PASS' } else { 'FAIL' }); Check = $Name; Detail = $Detail })
  Write-Host ("[{0}] {1} {2}" -f $(if ($Ok) { 'PASS' } else { 'FAIL' }), $Name, $Detail)
}

function Wait-Until([scriptblock]$Cond, [int]$Seconds) {
  $t = [Diagnostics.Stopwatch]::StartNew()
  while ($t.Elapsed.TotalSeconds -lt $Seconds) {
    if (& $Cond) { return $true }
    Start-Sleep -Milliseconds 500
  }
  return [bool](& $Cond)
}

function Get-BridgePort {
  foreach ($p in 38917..38927) {
    try {
      $r = Invoke-RestMethod -Uri "http://127.0.0.1:$p/v1/ping" -TimeoutSec 1
      if ($r.app -eq 'idm-replica') { return $p }
    } catch {}
  }
  return $null
}

function App-Running { [bool](Get-Process -Name $product -ErrorAction SilentlyContinue) }

# --- Preconditions ----------------------------------------------------------
if ((App-Running) -or (Get-Process -Name 'idm-replica' -ErrorAction SilentlyContinue)) { throw 'IDM Replica is running; quit it first.' }
if (Test-Path $uninstKey) { throw "$product is already installed; uninstall it first." }
if (Test-Path $desktopLnk) { throw "$desktopLnk already exists; remove it first." }
if ($null -ne (Get-BridgePort)) { throw 'Something already answers on the bridge port.' }

$runBefore = (Get-ItemProperty $runKey -Name $product -ErrorAction SilentlyContinue).$product
$stamp = Get-Date -Format 'yyyyMMddHHmmss'
$moved = @()
foreach ($d in @($roaming, $local)) {
  if (Test-Path $d) {
    $aside = "$d.smoke-backup-$stamp"
    Rename-Item -LiteralPath $d -NewName (Split-Path -Leaf $aside)
    $moved += , @($d, $aside)
    Write-Host "moved your data aside: $aside"
  }
}

try {
  # --- 1. Install ------------------------------------------------------------
  $t = [Diagnostics.Stopwatch]::StartNew()
  $p = Start-Process -FilePath $Installer -ArgumentList '/S', "/D=$dir" -PassThru -Wait
  Check 'silent install exits 0' ($p.ExitCode -eq 0) ("exit {0}, {1:N1} s" -f $p.ExitCode, $t.Elapsed.TotalSeconds)
  Check 'app exe installed' (Test-Path $exe) $exe
  Check 'uninstaller installed' (Test-Path (Join-Path $dir 'uninstall.exe'))
  Check 'unpacked extension installed' (Test-Path (Join-Path $dir 'resources\extension\manifest.json'))
  $bundled = Test-Path (Join-Path $dir 'resources\components\yt-dlp\yt-dlp.exe')
  Write-Host ("variant: {0}" -f $(if ($bundled) { 'FULL (tools bundled)' } else { 'LITE (tools downloaded on first run)' }))
  Check 'uninstall registry key' (Test-Path $uninstKey) ((Get-ItemProperty $uninstKey -ErrorAction SilentlyContinue).DisplayVersion)
  Check 'start menu shortcut' (Test-Path (Join-Path $startMenu "$product.lnk"))
  Check 'desktop shortcut (silent installs create it)' (Test-Path $desktopLnk)

  # --- 2. Launch with a minimal PATH ------------------------------------------
  $psi = New-Object Diagnostics.ProcessStartInfo $exe
  $psi.UseShellExecute = $false
  $psi.EnvironmentVariables['PATH'] = "$env:SystemRoot\System32;$env:SystemRoot;$env:SystemRoot\System32\WindowsPowerShell\v1.0"
  $psi.EnvironmentVariables['IDM_REPLICA_TEST_HOOKS'] = '1'
  $app = [Diagnostics.Process]::Start($psi)
  $port = $null
  $ok = Wait-Until { $script:port = Get-BridgePort; $null -ne $script:port } 60
  Check 'bridge answers /v1/ping' $ok "port $port"

  $m = Get-Content -Raw (Join-Path $here '..\src-tauri\components.json') | ConvertFrom-Json
  $compDir = Join-Path $local 'components'
  $ok = Wait-Until {
    @($m.components | Where-Object { -not (Test-Path (Join-Path $compDir "$($_.id)\$($_.exe)")) }).Count -eq 0
  } $TimeoutSec
  Check 'components resolve from the components folder' $ok $compDir
  foreach ($c in $m.components | Where-Object { $null -eq $_.archive }) {
    $f = Join-Path $compDir "$($c.id)\$($c.exe)"
    $h = if (Test-Path $f) { (Get-FileHash -Algorithm SHA256 $f).Hash.ToLowerInvariant() } else { '(missing)' }
    Check "$($c.id) matches its pinned SHA-256" ($h -eq $c.sha256) $h
  }
  $ff = Join-Path $compDir 'ffmpeg\ffmpeg.exe'
  $ffOk = (Test-Path $ff) -and ((& $ff -version 2>$null | Select-Object -First 1) -match '^ffmpeg version')
  Check 'ffmpeg runs (-version)' $ffOk
  if ($bundled) {
    Check 'FULL: bundled tools were moved out of the install folder' (-not (Test-Path (Join-Path $dir 'resources\components\yt-dlp\yt-dlp.exe')))
  }

  # --- 3. "Start with Windows" on (as the app writes it) ----------------------
  New-ItemProperty -Path $runKey -Name $product -Value "`"$exe`" --autostart" -PropertyType String -Force | Out-Null

  # --- 4. Quit, uninstall, leftovers -----------------------------------------
  $q = New-Object Diagnostics.ProcessStartInfo -ArgumentList $exe, '--test-tray=quit'
  $q.UseShellExecute = $false
  $q.EnvironmentVariables['IDM_REPLICA_TEST_HOOKS'] = '1'
  [void][Diagnostics.Process]::Start($q)
  Check 'tray Quit ends the app' (Wait-Until { -not (App-Running) } 20)
  # WebView2 helper processes can hold files in the data dir for a moment.
  Wait-Until { -not (Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object { $_.CommandLine -match [regex]::Escape($bundleId) }) } 20 | Out-Null

  $u = Start-Process -FilePath (Join-Path $dir 'uninstall.exe') -ArgumentList '/S', '/DELETEAPPDATA' -PassThru -Wait
  Check 'silent uninstall starts' ($u.ExitCode -eq 0) "exit $($u.ExitCode)"
  # The uninstaller re-runs itself from %TEMP%; wait for its work to finish.
  Wait-Until { -not (Test-Path $uninstKey) -and -not (Test-Path $dir) } 60 | Out-Null

  Check 'install folder removed' (-not (Test-Path $dir)) $(if (Test-Path $dir) { (Get-ChildItem -Recurse $dir | ForEach-Object FullName) -join '; ' })
  Check 'uninstall registry key removed' (-not (Test-Path $uninstKey))
  Check 'install-location registry key removed' (-not (Test-Path $manuKey))
  Check 'empty publisher key removed' (-not (Test-Path "HKCU:\Software\$publisher"))
  Check 'Start-with-Windows Run value removed' ($null -eq (Get-ItemProperty $runKey -Name $product -ErrorAction SilentlyContinue))
  Check 'start menu shortcut removed' (-not (Test-Path $startMenu))
  Check 'desktop shortcut removed' (-not (Test-Path $desktopLnk))
  Check 'app data removed (/DELETEAPPDATA)' (-not (Test-Path $roaming) -and -not (Test-Path $local)) "$roaming ; $local"
} finally {
  if (App-Running) { Stop-Process -Name $product -Force -ErrorAction SilentlyContinue }
  Remove-ItemProperty -Path $runKey -Name $product -ErrorAction SilentlyContinue
  if ($runBefore) { New-ItemProperty -Path $runKey -Name $product -Value $runBefore -PropertyType String -Force | Out-Null }
  foreach ($pair in $moved) {
    $orig, $aside = $pair
    if (Test-Path $orig) { Remove-Item -Recurse -Force $orig -ErrorAction SilentlyContinue }
    Rename-Item -LiteralPath $aside -NewName (Split-Path -Leaf $orig)
    Write-Host "restored your data: $orig"
  }
}

$failed = @($results | Where-Object Result -eq 'FAIL').Count
Write-Host ("`n{0} checks, {1} failed" -f $results.Count, $failed)
exit $(if ($failed) { 1 } else { 0 })
