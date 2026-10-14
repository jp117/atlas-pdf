# Atlas fork: install Atlas PDF for the current user (no administrator rights needed).
#
#   powershell -ExecutionPolicy Bypass -File packaging\windows\install-atlas.ps1            # install or update
#   powershell -ExecutionPolicy Bypass -File packaging\windows\install-atlas.ps1 -Uninstall
#
# Copies a release build to %LOCALAPPDATA%\Programs\Atlas PDF (the app as AtlasPDF.exe, so it
# never collides with an installed PdfCraft's pdfcraft.exe), adds a Start menu shortcut and an
# "Open with Atlas PDF" entry on PDFs, and registers Atlas PDF with Windows' Default Programs
# (a ProgID, OpenWithProgids, Capabilities and RegisteredApplications, as PdfCraft's installer
# does). Windows does not let a program make itself the default: this only offers Atlas PDF,
# and the script opens Settings so you can choose it. Your settings (%APPDATA%\AtlasPDF) are
# left alone by -Uninstall.

param(
    [string]$BinDir = (Join-Path $PSScriptRoot '..\..\target\release'),
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'
$Name = 'Atlas PDF'
$Dest = Join-Path $env:LOCALAPPDATA 'Programs\Atlas PDF'
$Exe = Join-Path $Dest 'AtlasPDF.exe'
$ProgId = 'AtlasPDF.Document'
$Classes = 'HKCU:\Software\Classes'
$Capabilities = 'HKCU:\Software\AtlasPDF\Capabilities'
$Shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Atlas PDF.lnk'
$Repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')

# Every key this script creates, removed whole by -Uninstall.
$OwnKeys = @(
    "$Classes\$ProgId",
    "$Classes\Applications\AtlasPDF.exe",
    "$Classes\SystemFileAssociations\.pdf\shell\AtlasPDF.Open",
    'HKCU:\Software\AtlasPDF',
    'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\AtlasPDF.exe'
)

# Create a key only when it is missing: New-Item -Force on an existing registry key deletes and
# recreates it, which would wipe other apps' values in shared keys (.pdf\OpenWithProgids,
# RegisteredApplications).
function Confirm-Key([string]$Key) {
    if (-not (Test-Path $Key)) { New-Item -Path $Key -Force | Out-Null }
}

function Set-Default([string]$Key, [string]$Value) {
    Confirm-Key $Key
    Set-Item -Path $Key -Value $Value
}

function Set-Value([string]$Key, [string]$ValueName, [string]$Value) {
    Confirm-Key $Key
    New-ItemProperty -Path $Key -Name $ValueName -Value $Value -PropertyType String -Force | Out-Null
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
    foreach ($key in $OwnKeys) {
        if (Test-Path $key) { Remove-Item -Path $key -Recurse -Force }
    }
    $openWith = "$Classes\.pdf\OpenWithProgids"
    if (Test-Path $openWith) { Remove-ItemProperty -Path $openWith -Name $ProgId -ErrorAction SilentlyContinue }
    Remove-ItemProperty -Path 'HKCU:\Software\RegisteredApplications' -Name $Name -ErrorAction SilentlyContinue
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

# Default Programs registration. The document icon is resource 2 of the executable (build.rs).
$open = "`"$Exe`" `"%1`""
Set-Default "$Classes\$ProgId" 'PDF Document'
Set-Default "$Classes\$ProgId\DefaultIcon" "`"$Exe`",-2"
Set-Default "$Classes\$ProgId\shell\open\command" $open
Set-Value "$Classes\.pdf\OpenWithProgids" $ProgId ''
Set-Value "$Classes\Applications\AtlasPDF.exe" 'FriendlyAppName' $Name
Set-Default "$Classes\Applications\AtlasPDF.exe\DefaultIcon" "`"$Exe`",-2"
Set-Value "$Classes\Applications\AtlasPDF.exe\SupportedTypes" '.pdf' ''
Set-Default "$Classes\Applications\AtlasPDF.exe\shell\open\command" $open
Set-Value $Capabilities 'ApplicationName' $Name
Set-Value $Capabilities 'ApplicationDescription' "Atlas Switch's PDF workbench for construction drawings"
Set-Value $Capabilities 'ApplicationIcon' "`"$Exe`",0"
Set-Value "$Capabilities\FileAssociations" '.pdf' $ProgId
Set-Value 'HKCU:\Software\RegisteredApplications' $Name 'Software\AtlasPDF\Capabilities'
Set-Default 'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\AtlasPDF.exe' $Exe

# "Open with Atlas PDF" on every PDF, whichever app is the default (Show more options on Windows 11).
$menu = "$Classes\SystemFileAssociations\.pdf\shell\AtlasPDF.Open"
Set-Default $menu 'Open with Atlas PDF'
Set-Value $menu 'Icon' "`"$Exe`",0"
Set-Value $menu 'MultiSelectModel' 'Document'
Set-Default "$menu\command" $open

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
Write-Host 'To make it the default for PDFs: in the Settings page that opens, set .pdf to Atlas PDF.'
Start-Process 'ms-settings:defaultapps?registeredAppUser=Atlas%20PDF'
