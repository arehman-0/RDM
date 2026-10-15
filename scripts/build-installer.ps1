<#
.SYNOPSIS
  Build the Windows installer (NSIS, per-user) in one command.

.DESCRIPTION
  -Variant lite (default): no video tools inside; the app's first-run setup
     downloads yt-dlp, QuickJS and FFmpeg and checks their SHA-256.
  -Variant full: also ships the pinned tools (scripts\fetch-components.ps1,
     hash-verified); the app moves them into its components folder on first
     run so yt-dlp stays updatable.

  Both variants ship the unpacked browser extension (for "Manual install").
  Output: release\IDM-Replica-<version>-x64-<variant>-setup.exe and
  release\SHA256SUMS.txt (one line per installer in release\).

  Signing is off unless IDM_SIGN=1 (see docs\SIGNING.md).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1 -Variant lite
  powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1 -Variant full
#>
[CmdletBinding()]
param(
  [ValidateSet('lite', 'full')][string]$Variant = 'lite'
)

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$tauri = Join-Path $root 'src-tauri'
$work = Join-Path $tauri 'target\installer'
$res = Join-Path $tauri 'resources'
Set-Location $root

function Invoke-Checked([string]$what, [scriptblock]$cmd) {
  & $cmd
  if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

function Write-Utf8([string]$Path, [string]$Text, [switch]$Bom) {
  [IO.File]::WriteAllText($Path, $Text, (New-Object Text.UTF8Encoding($Bom.IsPresent)))
}

$conf = Get-Content -Raw (Join-Path $tauri 'tauri.conf.json') | ConvertFrom-Json
$version = $conf.package.version
Invoke-Checked 'version check' { node scripts\check-versions.mjs }
New-Item -ItemType Directory -Force -Path $work | Out-Null

# 1. The unpacked extension (the store zip is built separately).
if (-not (Test-Path 'extension\node_modules')) { Invoke-Checked 'extension npm ci' { npm --prefix extension ci } }
Invoke-Checked 'extension build' { npm --prefix extension run build }
$extOut = Join-Path $res 'extension'
if (Test-Path $extOut) { Remove-Item -Recurse -Force $extOut }
New-Item -ItemType Directory -Force -Path $res | Out-Null
Copy-Item -Recurse 'extension\dist' $extOut
Get-ChildItem -Recurse -File $extOut -Filter *.map | Remove-Item -Force

# 2. The video tools (FULL only).
$compOut = Join-Path $res 'components'
if ($Variant -eq 'full') {
  Invoke-Checked 'fetch-components' { powershell -NoProfile -ExecutionPolicy Bypass -File scripts\fetch-components.ps1 -Dest $compOut }
} elseif (Test-Path $compOut) {
  Remove-Item -Recurse -Force $compOut
}

# 3. License page: the project's license + every third-party notice.
$license = Join-Path $work 'license.txt'
$text = (Get-Content -Raw LICENSE) + "`r`n`r`n" + ('=' * 72) + "`r`n`r`n" + (Get-Content -Raw -Encoding UTF8 THIRD_PARTY_NOTICES.md)
Write-Utf8 $license $text -Bom

# 4. The NSIS template, with the store links from extension-store.json.
$store = Get-Content -Raw (Join-Path $tauri 'extension-store.json') | ConvertFrom-Json
$nsi = Get-Content -Raw -Encoding UTF8 (Join-Path $tauri 'installer\installer.nsi')
$nsi = $nsi.Replace('__EXT_CHROME_STORE_URL__', [string]$store.chromeWebStoreUrl).Replace('__EXT_EDGE_STORE_URL__', [string]$store.edgeAddonsUrl)
if ($nsi.Contains('__EXT_')) { throw 'unreplaced placeholder in installer.nsi' }
$template = Join-Path $work 'installer.nsi'
Write-Utf8 $template $nsi

# 5. Variant config, merged over tauri.conf.json by the Tauri CLI.
$resources = @('resources/extension/**/*')
if ($Variant -eq 'full') { $resources += 'resources/components/**/*' }
$windows = @{ nsis = @{ template = $template; license = $license } }
if ($env:IDM_SIGN -eq '1') {
  # docs\SIGNING.md: a certificate in the user's store, picked by thumbprint.
  if (-not $env:IDM_CERT_THUMBPRINT) { throw 'IDM_SIGN=1 needs IDM_CERT_THUMBPRINT' }
  $windows.certificateThumbprint = $env:IDM_CERT_THUMBPRINT
  $windows.digestAlgorithm = 'sha256'
  $windows.timestampUrl = if ($env:IDM_TIMESTAMP_URL) { $env:IDM_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
}
$override = @{ tauri = @{ bundle = @{ resources = $resources; windows = $windows } } }
$overrideFile = Join-Path $work "tauri.$Variant.conf.json"
Write-Utf8 $overrideFile ($override | ConvertTo-Json -Depth 8)

# 6. Build (runs `npm run build` for the UI first, then cargo, then NSIS).
Invoke-Checked 'tauri build' { npx tauri build --config $overrideFile }

# 7. Name it by variant and refresh the checksum list.
$built = Join-Path $tauri "target\release\bundle\nsis\$($conf.package.productName)_$($version)_x64-setup.exe"
if (-not (Test-Path $built)) { throw "installer not found: $built" }
New-Item -ItemType Directory -Force -Path 'release' | Out-Null
$final = Join-Path $root "release\IDM-Replica-$version-x64-$Variant-setup.exe"
Copy-Item -Force $built $final
$sums = Get-ChildItem 'release' -Filter '*-setup.exe' | Sort-Object Name | ForEach-Object {
  '{0}  {1}' -f (Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLowerInvariant(), $_.Name
}
Write-Utf8 (Join-Path $root 'release\SHA256SUMS.txt') (($sums -join "`n") + "`n")
Write-Host ('built {0} ({1:N2} MB)' -f $final, ((Get-Item $final).Length / 1MB))
