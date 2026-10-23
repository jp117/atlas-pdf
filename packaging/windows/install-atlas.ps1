# Atlas fork: install Atlas PDF for the current user (no administrator rights needed).
#
#   powershell -ExecutionPolicy Bypass -File packaging\windows\install-atlas.ps1            # install or update
#   powershell -ExecutionPolicy Bypass -File packaging\windows\install-atlas.ps1 -Uninstall
#
# For trying a local build: everyone else installs AtlasPDF-win-Setup.exe from the GitHub
# release, which updates itself (docs/releasing-atlas.md). Don't keep both: they register the same
# names, and whichever ran last owns them.
#
# Copies a release build to %LOCALAPPDATA%\Programs\Atlas PDF (the app as AtlasPDF.exe, so it
# never collides with an installed PdfCraft's pdfcraft.exe) and adds a Start menu shortcut. The
# app registers itself, through the hooks Setup runs (apps/pdfcraft/src/atlas_register.rs): an
# "Open with Atlas PDF" entry on PDFs and Windows' Default Programs. Windows does not let a program
# make itself the default: this only offers Atlas PDF, and a first install opens Settings so you
# can choose it. Your settings (%APPDATA%\AtlasPDF) are left alone by -Uninstall.

param(
    [string]$BinDir = (Join-Path $PSScriptRoot '..\..\target\release'),
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'
$Name = 'Atlas PDF'
$Dest = Join-Path $env:LOCALAPPDATA 'Programs\Atlas PDF'
$Exe = Join-Path $Dest 'AtlasPDF.exe'
$Capabilities = 'HKCU:\Software\AtlasPDF\Capabilities'
$Shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Atlas PDF.lnk'
$Repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')

# Run one of the app's Setup hooks (--veloapp-install or --veloapp-uninstall; Velopack's names),
# which register or unregister it and exit. The version argument is required but unused.
function Invoke-Hook([string]$Hook) {
    Start-Process -FilePath $Exe -ArgumentList $Hook, '0.0.0' -Wait -NoNewWindow
}

# Tell Explorer that file associations changed, so menus and icons update without a sign-out.
function Send-AssociationChanged {
    Add-Type -Namespace AtlasPdf -Name Shell -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("shell32.dll")]
public static extern void SHChangeNotify(int eventId, uint flags, System.IntPtr item1, System.IntPtr item2);
'@
    [AtlasPdf.Shell]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)
}

function Test-Running {
    $running = Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path -like "$Dest\*" }
    if ($running) {
        throw "$Name is running from $Dest. Close it (save your work first), then run this again."
    }
}

if ($Uninstall) {
    Test-Running
    if (Test-Path $Exe) { Invoke-Hook '--veloapp-uninstall' }
    if (Test-Path $Shortcut) { Remove-Item $Shortcut -Force }
    if (Test-Path $Dest) { Remove-Item $Dest -Recurse -Force }
    Send-AssociationChanged
    Write-Host "Removed $Name. Your settings in $env:APPDATA\AtlasPDF are kept."
    return
}

# Files.
$app = Join-Path $BinDir 'pdfcraft.exe'
$cli = Join-Path $BinDir 'pdfcraft-cli.exe'
foreach ($file in @($app, $cli)) {
    if (-not (Test-Path $file)) { throw "Missing $file. Build first: cargo build --release -p pdfcraft -p pdfcraft-cli" }
}
Test-Running
New-Item -ItemType Directory -Force $Dest | Out-Null
Copy-Item $app $Exe -Force
Copy-Item $cli (Join-Path $Dest 'pdfcraft-cli.exe') -Force
foreach ($notice in @('LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'assets\app-icon\LICENSE-document.txt')) {
    Copy-Item (Join-Path $Repo $notice) $Dest -Force
}
# OCR models (cargo xtask models), where the app looks for them: beside the executable.
$models = Join-Path $Repo 'assets\models'
if (Test-Path (Join-Path $models '*.rten')) {
    New-Item -ItemType Directory -Force (Join-Path $Dest 'models') | Out-Null
    Copy-Item (Join-Path $models '*') (Join-Path $Dest 'models') -Force
} else {
    Write-Warning 'No OCR models in assets\models (run: cargo xtask models). Everything but Scan & OCR works.'
}

# Default Programs registration, by the app itself.
$firstInstall = -not (Test-Path $Capabilities)
Invoke-Hook '--veloapp-install'
if (-not (Test-Path $Capabilities)) { throw "$Exe didn't register itself: build it from a commit that has apps/pdfcraft/src/atlas_register.rs." }

# Start menu shortcut.
$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut($Shortcut)
$link.TargetPath = $Exe
$link.WorkingDirectory = $Dest
$link.Description = "Atlas Switch's PDF workbench"
$link.IconLocation = "$Exe,0"
$link.Save()

Send-AssociationChanged
Write-Host "Installed $Name in $Dest."
# Only a first install asks about the PDF default; an update keeps whatever the user chose.
if ($firstInstall) {
    Write-Host 'To make it the default for PDFs: in the Settings page that opens, set .pdf to Atlas PDF.'
    Start-Process 'ms-settings:defaultapps?registeredAppUser=Atlas%20PDF'
}
