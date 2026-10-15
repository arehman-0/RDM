<#
.SYNOPSIS
  Download the pinned video tools listed in src-tauri/components.json, verify
  each SHA-256, and lay them out for the FULL installer.

.DESCRIPTION
  Output: <Dest>\<id>\<exe> (+ the DLLs FFmpeg needs), the same layout the
  app's resolver looks for under <install dir>\resources\components.
  Downloads are cached in -Cache, re-verified every run, and deleted on a
  hash mismatch. Any mismatch or missing file stops the script with an error.
  Nothing here is ever committed to git (see .gitignore).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\fetch-components.ps1
#>
[CmdletBinding()]
param(
  [string]$Manifest,
  [string]$Dest,
  [string]$Cache
)

$ErrorActionPreference = 'Stop'
# (Windows PowerShell 5.1 leaves $PSScriptRoot empty inside param defaults.)
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $Manifest) { $Manifest = Join-Path $here '..\src-tauri\components.json' }
if (-not $Dest) { $Dest = Join-Path $here '..\src-tauri\resources\components' }
if (-not $Cache) { $Cache = Join-Path $here '..\src-tauri\target\component-cache' }
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is very slow with the progress bar
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Get-Sha256([string]$Path) {
  (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

$m = Get-Content -Raw -LiteralPath $Manifest | ConvertFrom-Json
if ($m.version -ne 1) { throw "unsupported components.json version $($m.version)" }
New-Item -ItemType Directory -Force -Path $Cache | Out-Null
if (Test-Path $Dest) { Remove-Item -Recurse -Force $Dest }
New-Item -ItemType Directory -Force -Path $Dest | Out-Null

foreach ($c in $m.components) {
  if ($c.url -notmatch '^https://') { throw "$($c.id): URL must be https: $($c.url)" }
  $file = Join-Path $Cache ("{0}-{1}{2}" -f $c.id, $c.sha256.Substring(0, 12), [IO.Path]::GetExtension(([Uri]$c.url).AbsolutePath))

  if ((Test-Path $file) -and (Get-Sha256 $file) -ne $c.sha256.ToLowerInvariant()) {
    Write-Warning "$($c.id): cached file has the wrong hash; downloading again"
    Remove-Item -Force $file
  }
  if (-not (Test-Path $file)) {
    Write-Host "downloading $($c.id) $($c.version) ($([math]::Round($c.size / 1MB, 1)) MB) from $($c.url)"
    Invoke-WebRequest -UseBasicParsing -Uri $c.url -OutFile "$file.part"
    Move-Item -Force "$file.part" $file
  }

  $got = Get-Sha256 $file
  if ($got -ne $c.sha256.ToLowerInvariant()) {
    Remove-Item -Force $file
    throw "SHA-256 MISMATCH for $($c.id): expected $($c.sha256), got $got. The download was deleted."
  }
  if ((Get-Item $file).Length -ne $c.size) {
    throw "$($c.id): size $((Get-Item $file).Length) != pinned $($c.size)"
  }
  Write-Host "verified $($c.id) sha256=$got"

  $out = Join-Path $Dest $c.id
  New-Item -ItemType Directory -Force -Path $out | Out-Null
  if ($null -eq $c.archive) {
    Copy-Item -LiteralPath $file -Destination (Join-Path $out $c.exe)
  } else {
    # Same rule as components.rs::extract: <top folder>\<archive.dir>\<name>,
    # where name equals an include entry or ends with one that starts with '.'.
    $tmp = Join-Path $Cache "$($c.id)-unzip"
    if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
    Expand-Archive -LiteralPath $file -DestinationPath $tmp
    $top = Get-ChildItem -LiteralPath $tmp -Directory
    if ($top.Count -ne 1) { throw "$($c.id): expected one top-level folder in the archive" }
    $from = Join-Path $top[0].FullName $c.archive.dir
    foreach ($f in Get-ChildItem -LiteralPath $from -File) {
      $wanted = $c.archive.include | Where-Object { $f.Name -eq $_ -or ($_.StartsWith('.') -and $f.Name.EndsWith($_)) }
      if ($wanted) { Copy-Item -LiteralPath $f.FullName -Destination $out }
    }
    Remove-Item -Recurse -Force $tmp
  }
  if (-not (Test-Path (Join-Path $out $c.exe))) { throw "$($c.id): $($c.exe) missing after unpacking" }
}

$total = (Get-ChildItem -Recurse -File $Dest | Measure-Object Length -Sum).Sum
Write-Host ("components ready in {0} ({1:N1} MB)" -f (Resolve-Path $Dest), ($total / 1MB))
