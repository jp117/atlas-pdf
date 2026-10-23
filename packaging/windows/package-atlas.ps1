<#
.SYNOPSIS
  Atlas fork: build Atlas PDF's per-user Windows installer and update package (Velopack).

.DESCRIPTION
  Produces, in -Out (default: dist\atlas):
    AtlasSwitch.AtlasPDF-win-Setup.exe        the installer: per user, no administrator rights; Start menu
                                              and desktop shortcuts; registers Atlas PDF for PDFs
    AtlasSwitch.AtlasPDF-<version>-full.nupkg the package installed copies update from
    releases.win.json                         the feed installed copies read (apps/pdfcraft/src/atlas_install.rs)
    AtlasSwitch.AtlasPDF-win-Portable.zip     the same app, unzipped and run anywhere (no automatic updates)

  The version is [workspace.package] version in Cargo.toml (`cargo xtask version`). The binaries
  link the C runtime statically, so no Visual C++ redistributable is needed. `atlas-release.yml`
  runs this and uploads the result to a GitHub Release; see docs/releasing-atlas.md.

  Needs: Rust (MSVC toolchain), the network (the OCR models), and vpk at the same version as the
  velopack crate in apps/pdfcraft/Cargo.toml (.NET 8 SDK, then:
  dotnet tool install -g vpk --version 1.2.161).

.EXAMPLE
  pwsh packaging/windows/package-atlas.ps1
  pwsh packaging/windows/package-atlas.ps1 -SkipBuild
#>
param(
  [string] $Out = '',
  [switch] $SkipBuild
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Target = 'x86_64-pc-windows-msvc'
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
if (-not $Out) { $Out = Join-Path $Root 'dist\atlas' }

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

Push-Location $Root
try {
  $Version = (cargo xtask version | Select-Object -Last 1).Trim()
  if ($LASTEXITCODE -ne 0 -or $Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') { throw "unexpected version '$Version' from cargo xtask version" }
  Write-Output "Atlas PDF $Version for Windows x64"

  if (-not $env:PDFCRAFT_BUILD_SHA) { $env:PDFCRAFT_BUILD_SHA = (git rev-parse HEAD 2>$null) }
  if (-not $env:PDFCRAFT_BUILD_DATE) { $env:PDFCRAFT_BUILD_DATE = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd') }
  if (-not $SkipBuild) {
    # Static CRT, scoped to the target so build scripts and proc-macros are unaffected.
    [Environment]::SetEnvironmentVariable('CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS', '-C target-feature=+crt-static')
    # Fail rather than ship an exe without its icon and version resources.
    $env:PDFCRAFT_REQUIRE_WINRES = '1'
    Invoke-Native 'cargo build' { cargo build --release --locked -p pdfcraft -p pdfcraft-cli --target $Target }
  }
  $Bin = Join-Path $TargetDir "$Target\release"

  # The app must be a GUI program (PE subsystem 2: no console window opens with it), the CLI a
  # console one (3), both for x64.
  foreach ($check in @(@('pdfcraft.exe', 2), @('pdfcraft-cli.exe', 3))) {
    $bytes = [System.IO.File]::ReadAllBytes((Join-Path $Bin $check[0]))
    $pe = [BitConverter]::ToInt32($bytes, 0x3C)
    $machine = [BitConverter]::ToUInt16($bytes, $pe + 4)
    $subsystem = [BitConverter]::ToUInt16($bytes, $pe + 0x5C)
    if ($machine -ne 0x8664 -or $subsystem -ne $check[1]) { throw "$($check[0]): machine 0x$('{0:X}' -f $machine), subsystem $subsystem" }
  }

  # The app is AtlasPDF.exe, so it never collides with an installed PdfCraft's pdfcraft.exe.
  $Stage = Join-Path $TargetDir 'atlas-package'
  Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force -Path $Stage | Out-Null
  Copy-Item (Join-Path $Bin 'pdfcraft.exe') (Join-Path $Stage 'AtlasPDF.exe')
  Copy-Item (Join-Path $Bin 'pdfcraft-cli.exe') $Stage
  foreach ($f in 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'assets\app-icon\LICENSE-document.txt') {
    Copy-Item (Join-Path $Root $f) $Stage
  }
  # Embedded craft-fonts families (CRAFT_FONTS_DIR) ship their licences.
  if ($env:CRAFT_FONTS_DIR) {
    Get-ChildItem -Path (Join-Path $env:CRAFT_FONTS_DIR 'fonts') -Directory -ErrorAction SilentlyContinue | ForEach-Object {
      $ofl = Join-Path $_.FullName 'OFL.txt'
      if (Test-Path $ofl) { Copy-Item $ofl (Join-Path $Stage "OFL-$($_.Name).txt") }
    }
  }
  # OCR models, where the app looks for them: models\ beside the executable.
  $Models = Join-Path $Stage 'models'
  Invoke-Native 'cargo xtask models' { cargo xtask models $Models }
  Remove-Item -Force (Join-Path $Models '*.part') -ErrorAction SilentlyContinue
  if (-not (Test-Path (Join-Path $Models 'ATTRIBUTION.txt'))) { throw "no OCR models in $Models after cargo xtask models" }

  New-Item -ItemType Directory -Force -Path $Out | Out-Null
  # Full packages only (no deltas): installed copies read `releases/latest/download`, which holds
  # the newest release's files and none of the older ones a delta chain would need.
  # Setup installs into %LOCALAPPDATA%\<packId>, and uninstalling deletes it: never AtlasPDF, the
  # app's own folder there (crash recovery, single instance).
  Invoke-Native 'vpk pack' {
    vpk pack --packId AtlasSwitch.AtlasPDF --packVersion $Version --packDir $Stage --mainExe AtlasPDF.exe `
      --packTitle 'Atlas PDF' --packAuthors 'Atlas Switch' --icon (Join-Path $Root 'assets\app-icon\pdfcraft.ico') `
      --delta None --outputDir $Out
  }
  Get-ChildItem $Out | Format-Table Name, Length
} finally {
  Pop-Location
}
