<#
.SYNOPSIS
  Build, sign and package Tinta Correo for Windows.

.DESCRIPTION
  Produces, in $env:DIST (default: dist/release):
    tinta-correo-<version>-windows-<arch>.msi            per-machine installer (WiX v5)
    tinta-correo-<version>-windows-<arch>-portable.zip   ibercorreo.exe + ibercorreo-cli.exe + licences

  The binaries link the C runtime statically (+crt-static), so neither needs the Visual C++
  redistributable. Signing goes through sign.ps1, which leaves the files unsigned (with a warning)
  when no signing certificate is configured.

  Gmail and Microsoft 365 sign-in need the OAuth client ids compiled in (docs/oauth.md): set
  IBERCORREO_GOOGLE_CLIENT_ID, IBERCORREO_GOOGLE_CLIENT_SECRET and IBERCORREO_MICROSOFT_CLIENT_ID
  before building. Without them the build still works with password (IMAP/SMTP) accounts, and
  this script says so.

  Needs: Rust (MSVC toolchain + the target), the Windows SDK (signtool.exe, only to sign), and
  WiX v5: dotnet tool install --global wix --version 5.0.2

.EXAMPLE
  pwsh packaging/windows/package.ps1 -Arch x64
  pwsh packaging/windows/package.ps1 -Arch x64 -SkipBuild
#>
param(
  [ValidateSet('x64', 'arm64')] [string] $Arch = 'x64',
  [switch] $SkipBuild
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$AppTitle = 'Tinta Correo'
$AppSlug = 'tinta-correo'

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

# The version lives in one place: [workspace.package] version in the root Cargo.toml.
$Version = $env:IBERCORREO_VERSION
if (-not $Version) {
  $inPkg = $false
  foreach ($line in Get-Content (Join-Path $Root 'Cargo.toml')) {
    if ($line -match '^\s*\[') { $inPkg = ($line.Trim() -eq '[workspace.package]'); continue }
    if ($inPkg -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
  }
}
if (-not $Version) { throw 'could not read [workspace.package] version from Cargo.toml' }
# MSI ProductVersion is numeric (major.minor.build); pre-release tags are dropped there.
$MsiVersion = ($Version -split '-')[0]

$Target = switch ($Arch) { 'x64' { 'x86_64-pc-windows-msvc' } 'arm64' { 'aarch64-pc-windows-msvc' } }
$Dist = if ($env:DIST) { $env:DIST } else { Join-Path $Root 'dist\release' }
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

Write-Output "$AppTitle $Version for Windows $Arch ($Target)"
foreach ($v in 'IBERCORREO_GOOGLE_CLIENT_ID', 'IBERCORREO_MICROSOFT_CLIENT_ID') {
  if (-not [Environment]::GetEnvironmentVariable($v)) {
    Write-Warning "$v is not set: this build can't sign in to that provider with OAuth (docs/oauth.md); password accounts still work"
  }
}

if (-not $SkipBuild) {
  # Static CRT: no VC++ redistributable needed. Scoped to the target so host build scripts and
  # proc-macros are unaffected.
  $flagVar = 'CARGO_TARGET_' + ($Target.ToUpper() -replace '-', '_') + '_RUSTFLAGS'
  [Environment]::SetEnvironmentVariable($flagVar, '-C target-feature=+crt-static')
  Invoke-Native "cargo build ($Target)" { cargo build --release --locked -p ibercorreo -p ibercorreo-cli --target $Target }
}

$Bin = Join-Path $TargetDir "$Target\release"

# Check both binaries' PE headers before packaging: the machine must match -Arch, the app must be
# a GUI program (2: no console window opens with it) and the CLI a console one (3).
function Get-PeHeader([string] $Path) {
  $bytes = [System.IO.File]::ReadAllBytes($Path)
  $pe = [BitConverter]::ToInt32($bytes, 0x3C)
  return @{ Machine = [BitConverter]::ToUInt16($bytes, $pe + 4); Subsystem = [BitConverter]::ToUInt16($bytes, $pe + 0x5C) }
}
$Machine = switch ($Arch) { 'x64' { 0x8664 } 'arm64' { 0xAA64 } }
foreach ($check in @(@('ibercorreo.exe', 2), @('ibercorreo-cli.exe', 3))) {
  $h = Get-PeHeader (Join-Path $Bin $check[0])
  if ($h.Machine -ne $Machine) { throw "$($check[0]) is for machine 0x$('{0:X}' -f $h.Machine), expected 0x$('{0:X}' -f $Machine) ($Arch)" }
  if ($h.Subsystem -ne $check[1]) { throw "$($check[0]) has PE subsystem $($h.Subsystem), expected $($check[1])" }
  Write-Output "ok $($check[0]): $Arch, PE subsystem $($h.Subsystem)"
}

$Stage = Join-Path $TargetDir "windows-package\$Arch"
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Stage | Out-Null
Copy-Item (Join-Path $Bin 'ibercorreo.exe'), (Join-Path $Bin 'ibercorreo-cli.exe') $Stage
foreach ($f in 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE') {
  Copy-Item (Join-Path $Root $f) $Stage
}

& (Join-Path $PSScriptRoot 'sign.ps1') (Join-Path $Stage 'ibercorreo.exe') (Join-Path $Stage 'ibercorreo-cli.exe')

# ---- MSI ---------------------------------------------------------------------------------------
$Msi = Join-Path $Dist "$AppSlug-$Version-windows-$Arch.msi"
Invoke-Native 'wix build' {
  wix build (Join-Path $PSScriptRoot 'tinta-correo.wxs') (Join-Path $PSScriptRoot 'installer-ui.wxs') -arch $Arch `
    -d "Version=$MsiVersion" -d "BinDir=$Stage" -d "IconPath=$(Join-Path $Root 'assets\app-icon\ibercorreo.ico')" `
    -o $Msi
}
# wix writes its debug symbols (.wixpdb) next to the MSI; keep them out of the release assets.
Remove-Item -Force -ErrorAction SilentlyContinue ([IO.Path]::ChangeExtension($Msi, '.wixpdb'))
& (Join-Path $PSScriptRoot 'sign.ps1') $Msi

# ---- portable zip ------------------------------------------------------------------------------
$Portable = Join-Path $TargetDir "windows-package\$AppSlug-$Version-windows-$Arch-portable"
Remove-Item -Recurse -Force $Portable -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Portable | Out-Null
Copy-Item (Join-Path $Stage '*') $Portable
Copy-Item (Join-Path $Root 'README.md') $Portable
$Zip = Join-Path $Dist "$AppSlug-$Version-windows-$Arch-portable.zip"
Remove-Item -Force $Zip -ErrorAction SilentlyContinue
Compress-Archive -Path $Portable -DestinationPath $Zip

$HostArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
if ($Arch -ne 'arm64' -or $HostArch -eq 'arm64') {
  Invoke-Native 'ibercorreo-cli --version' { & (Join-Path $Stage 'ibercorreo-cli.exe') --version }
}
Get-Item $Msi, $Zip | Format-Table Name, Length
