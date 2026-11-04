# Atlas fork: what the printer rules send to real Windows printers, without using paper. For the
# command line and the Print window: each job is held on a paused queue, its DEVMODE read, converted
# to a print ticket by the driver and checked, then deleted, and the queue resumed.
#
#   pwsh packaging/windows/test-print-hold.ps1              # every case
#   pwsh packaging/windows/test-print-hold.ps1 -Only cli    # or -Only window
#
# It needs Atlas's three printers set up as docs/printing-atlas.md describes, and builds run first:
#   cargo build -p pdfcraft -p pdfcraft-cli
# ATLASPDF_PRINT_RULES points both at a test rules file, so the real rules are left alone.

param(
    [string]$Bin = (Join-Path $PSScriptRoot '..\..\target\debug'),
    [ValidateSet('all', 'cli', 'window')][string]$Only = 'all',
    # Only the cases whose names match this wildcard.
    [string]$Case = '*'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'print-hold-lib.ps1')

$canonD = 'Canon TX-3200'
$canonC = 'Canon TX-3200 18x24'
$office = 'HP LaserJet M607 [F35101]'
$cli = Join-Path $Bin 'pdfcraft-cli.exe'
$app = Join-Path $Bin 'pdfcraft.exe'
foreach ($exe in $cli, $app) { if (-not (Test-Path $exe)) { throw "$exe is missing: build it first" } }

$work = Join-Path $env:TEMP "atlas-print-hold-$PID"
New-Item -ItemType Directory -Force $work | Out-Null
$env:ATLASPDF_PRINT_RULES = Join-Path $work 'print-rules.json'

$pdf = @{
    cPortrait  = Join-Path $work 'c-portrait.pdf'
    cLandscape = Join-Path $work 'c-landscape.pdf'
    cCropped   = Join-Path $work 'c-cropped-rotated.pdf'
    dPortrait  = Join-Path $work 'd-portrait.pdf'
    dLandscape = Join-Path $work 'd-landscape.pdf'
    letter     = Join-Path $work 'letter.pdf'
    mixed      = Join-Path $work 'mixed.pdf'
    noRule     = Join-Path $work 'ansi-d.pdf'
    oddSize    = Join-Path $work 'odd-size.pdf'
}
New-AtlasTestPdf $pdf.cPortrait @(@{ w = 1296; h = 1728; label = 'HELD TEST 18x24 portrait' })
New-AtlasTestPdf $pdf.cLandscape @(@{ w = 1728; h = 1296; label = 'HELD TEST 18x24 landscape' })
New-AtlasTestPdf $pdf.cCropped @(@{ w = 3000; h = 3000; crop = @(100, 100, 1828, 1396); rotate = 270; label = 'HELD TEST crop and rotate' })
New-AtlasTestPdf $pdf.dPortrait @(@{ w = 1728; h = 2592; label = 'HELD TEST 24x36 portrait' })
New-AtlasTestPdf $pdf.dLandscape @(@{ w = 2592; h = 1728; label = 'HELD TEST 24x36 landscape' })
New-AtlasTestPdf $pdf.letter @(@{ w = 612; h = 792; label = 'HELD TEST letter' })
New-AtlasTestPdf $pdf.mixed @(
    @{ w = 1296; h = 1728; label = 'HELD TEST mixed 18x24' },
    @{ w = 612; h = 792; label = 'HELD TEST mixed letter' },
    @{ w = 1728; h = 1296; label = 'HELD TEST mixed 18x24 landscape' },
    @{ w = 1728; h = 2592; label = 'HELD TEST mixed 24x36' }
)
New-AtlasTestPdf $pdf.noRule @(@{ w = 1584; h = 2448; label = 'HELD TEST ANSI D' })
New-AtlasTestPdf $pdf.oddSize @(@{ w = 1440; h = 2160; label = 'HELD TEST 20x30' })

# What each queue must carry through: its saved source, roll width and rotate, on the job's paper.
$rollC = @{ JobInputBin = 'RollPaper2'; JobRollWidth = 'Roll24inch'; JobRotate90 = 'ON' }
$rollD = @{ JobInputBin = 'RollPaper1'; JobRollWidth = 'Roll36inch'; JobRotate90 = 'ON' }
$archC = @{ PageMediaSize = 'NorthAmericaArchitectureCSheet' }
$archD = @{ PageMediaSize = 'NorthAmericaArchitectureDSheet' }
$letterPaper = @{ PageMediaSize = 'NorthAmericaLetter' }

$failures = New-Object Collections.Generic.List[string]
$passed = 0

function Fail([string]$Case, [string]$Why) {
    $failures.Add("${Case}: $Why")
    Write-Host "  FAIL $Why" -ForegroundColor Red
}

# Check a held job against what's wanted; $Want keys are print ticket features or 'orientation'.
function Test-Job($Case, $Job, [hashtable[]]$Want, [string]$Orientation) {
    $ok = $true
    foreach ($part in $Want) {
        foreach ($key in $part.Keys) {
            if ($Job.ticket[$key] -ne $part[$key]) {
                Fail $Case "$($Job.printer): $key is $($Job.ticket[$key]), wanted $($part[$key])"
                $ok = $false
            }
        }
    }
    if ($Orientation -and $Job.devmode.orientation -ne $Orientation) {
        Fail $Case "$($Job.printer): orientation is $($Job.devmode.orientation), wanted $Orientation"
        $ok = $false
    }
    Write-Host ("  {0}: {1}, {2}, {3} {4}, rotate {5}, {6}" -f $Job.printer, $Job.devmode.form, $Job.devmode.orientation, $Job.ticket['JobInputBin'], $Job.ticket['JobRollWidth'], $Job.ticket['JobRotate90'], $Job.name)
    return $ok
}

# Run one case: $Body sends and checks; a failure in it is recorded and the next case still runs.
function Invoke-Case([string]$Name, [scriptblock]$Body) {
    if ($Name -notlike $Case) { return }
    Write-Host "--- $Name"
    $before = $failures.Count
    try { & $Body } catch { Fail $Name "$_" }
    if ($failures.Count -eq $before) { $script:passed++; Write-Host '  ok' -ForegroundColor Green }
}

function Write-Rules([object[]]$Rules) {
    if ($Rules) { Set-Content -Path $env:ATLASPDF_PRINT_RULES -Encoding UTF8 -Value (ConvertTo-Json @{ version = 1; rules = @($Rules) } -Depth 4) }
    elseif (Test-Path $env:ATLASPDF_PRINT_RULES) { Remove-Item $env:ATLASPDF_PRINT_RULES }
}

# --- the command line ---------------------------------------------------------------------------

function Invoke-Print([string]$File, [hashtable]$PrintArgs) {
    $steps = @(@{ tool = 'doc_open'; args = @{ path = $File } }, @{ tool = 'doc_print'; args = $PrintArgs + @{ doc = 1 } })
    $script = Join-Path $work 'steps.json'
    [IO.File]::WriteAllText($script, (ConvertTo-Json $steps -Depth 6))
    # The report is on standard output; a refused job also writes to standard error and exits 1.
    $ErrorActionPreference = 'Continue'
    $report = (& $cli run --script $script --report 2>$null | Out-String) | ConvertFrom-Json
    return $report.steps[1]
}

function Invoke-Auto([string]$File, [hashtable]$Extra = @{}) {
    return Invoke-Print $File (@{ paper = 'auto'; printer = 'auto' } + $Extra)
}

function Test-Reported($Case, $Step, [string]$Printer, [string]$Paper, [string]$Reason) {
    $r = $Step.result[0]
    if ($Step.status -ne 'ok') { Fail $Case "doc_print said: $($Step.error)"; return }
    if ($r.printer -ne $Printer) { Fail $Case "reported printer $($r.printer)" }
    if ($r.paper -ne $Paper) { Fail $Case "reported paper $($r.paper)" }
    if ($r.reason -ne $Reason) { Fail $Case "reported reason $($r.reason)" }
}

function Invoke-CliCases {
    Write-Rules @()
    $quiet = @{ $canonC = 0; $canonD = 0; $office = 0 }
    foreach ($c in @(
            @{ name = '18x24 portrait'; file = $pdf.cPortrait; printer = $canonC; paper = 'ARCH C (18 x 24 in)'; reason = "18 x 24 detected: $canonC"; want = @($rollC, $archC); o = 'Portrait' },
            @{ name = '18x24 landscape'; file = $pdf.cLandscape; printer = $canonC; paper = 'ARCH C (18 x 24 in)'; reason = "18 x 24 detected: $canonC"; want = @($rollC, $archC); o = 'Landscape' },
            @{ name = '18x24 by CropBox and Rotate'; file = $pdf.cCropped; printer = $canonC; paper = 'ARCH C (18 x 24 in)'; reason = "18 x 24 detected: $canonC"; want = @($rollC, $archC); o = 'Portrait' },
            @{ name = '24x36 portrait'; file = $pdf.dPortrait; printer = $canonD; paper = 'ARCH D (24 x 36 in)'; reason = "24 x 36 detected: $canonD"; want = @($rollD, $archD); o = 'Portrait' },
            @{ name = '24x36 landscape'; file = $pdf.dLandscape; printer = $canonD; paper = 'ARCH D (24 x 36 in)'; reason = "24 x 36 detected: $canonD"; want = @($rollD, $archD); o = 'Landscape' },
            @{ name = 'Letter'; file = $pdf.letter; printer = $office; paper = 'US Letter'; reason = "Letter detected: $office"; want = @(, $letterPaper); o = 'Portrait' }
        )) {
        Invoke-Case "cli: $($c.name)" {
            $step = $null
            $held = Invoke-HeldJob -Expect @{ $c.printer = 1 } -Send { $script:step = Invoke-Auto $c.file }
            Test-Reported "cli: $($c.name)" $script:step $c.printer $c.paper $c.reason
            [void](Test-Job "cli: $($c.name)" $held[0] $c.want $c.o)
        }
    }
    Invoke-Case 'cli: mixed sizes are refused unasked' {
        $held = Invoke-HeldJob -Expect $quiet -Send {
            $script:step = Invoke-Auto $pdf.mixed
        }
        if ($script:step.status -ne 'error' -or $script:step.error -notmatch 'mixed page sizes') { Fail 'cli: mixed' "doc_print said: $($script:step | ConvertTo-Json -Depth 4)" }
        if ($held.Count -ne 0) { Fail 'cli: mixed' 'a job was held' }
    }
    Invoke-Case 'cli: mixed sizes split into one job per printer' {
        $held = Invoke-HeldJob -Expect @{ $canonC = 1; $canonD = 1; $office = 1 } -Send { $script:step = Invoke-Auto $pdf.mixed @{ mixed = 'split' } }
        if ($script:step.status -ne 'ok') { Fail 'cli: split' $script:step.error }
        if ($script:step.result[0].jobs.Count -ne 3) { Fail 'cli: split' "reported $($script:step.result[0].jobs.Count) jobs" }
        foreach ($job in $held) {
            switch ($job.printer) {
                $canonC { [void](Test-Job 'cli: split' $job @($rollC, $archC) $null) }
                $canonD { [void](Test-Job 'cli: split' $job @($rollD, $archD) $null) }
                $office { [void](Test-Job 'cli: split' $job @(, $letterPaper) $null) }
            }
        }
    }
    Invoke-Case 'cli: no matching rule sends nothing' {
        [void](Invoke-HeldJob -Expect $quiet -Send { $script:step = Invoke-Auto $pdf.noRule })
        if ($script:step.status -ne 'error' -or $script:step.error -notmatch 'no printer rule') { Fail 'cli: no rule' "doc_print said: $($script:step | ConvertTo-Json -Depth 4)" }
    }
    Invoke-Case 'cli: a rule for a missing printer sends nothing' {
        Write-Rules @(@{ paper = 'ARCH C'; printer = 'No Such Printer' })
        try { [void](Invoke-HeldJob -Expect $quiet -Send { $script:step = Invoke-Auto $pdf.cPortrait }) }
        finally { Write-Rules @() }
        if ($script:step.status -ne 'error' -or $script:step.error -notmatch "isn't installed") { Fail 'cli: missing' "doc_print said: $($script:step | ConvertTo-Json -Depth 4)" }
    }
    Invoke-Case 'cli: a conflicting choice is refused' {
        [void](Invoke-HeldJob -Expect $quiet -Send { $script:step = Invoke-Print $pdf.cPortrait @{ paper = 'ARCH C'; printer = $canonD; layout = 'actual' } })
        if ($script:step.status -ne 'error' -or $script:step.error -notmatch 'allow_conflict') { Fail 'cli: conflict' "doc_print said: $($script:step | ConvertTo-Json -Depth 4)" }
    }
    Invoke-Case 'cli: an explicit job says when the rules could not be read' {
        Set-Content -Path $env:ATLASPDF_PRINT_RULES -Value '{ broken'
        try {
            [void](Invoke-HeldJob -Expect @{ $canonC = 1 } -Send { $script:step = Invoke-Print $pdf.cPortrait @{ paper = 'ARCH C'; printer = $canonC; layout = 'actual' } })
        } finally { Write-Rules @() }
        if ($script:step.result[0].rules_problem -notmatch "can't be used") { Fail 'cli: broken rules' "doc_print said: $($script:step | ConvertTo-Json -Depth 4)" }
    }
    Invoke-Case 'cli: a manual override goes where it was sent' {
        $held = Invoke-HeldJob -Expect @{ $canonD = 1; $canonC = 0 } -Send { $script:step = Invoke-Print $pdf.cPortrait @{ paper = 'ARCH C'; printer = $canonD; layout = 'actual'; allow_conflict = $true } }
        [void](Test-Job 'cli: override' $held[0] @($rollD, $archC) 'Portrait')
    }
}

# --- the Print window ---------------------------------------------------------------------------

$control = Join-Path $work 'control.txt'
$script:window = $null

function Ui {
    $out = & $cli ui --control $control @args 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) { throw "ui $($args -join ' '): $out" }
    return $out
}

function Start-Window([string]$File) {
    if (Test-Path $control) { Remove-Item $control }
    $script:window = Start-Process -FilePath $app -ArgumentList @('--control', "`"$control`"", "`"$File`"") -PassThru
    $deadline = (Get-Date).AddSeconds(60)
    while (-not (Test-Path $control)) {
        if ((Get-Date) -gt $deadline) { throw 'the window never offered its control channel' }
        Start-Sleep -Milliseconds 250
    }
    do {
        Start-Sleep -Milliseconds 500
        try { $state = Ui state | ConvertFrom-Json } catch { $state = $null }
    } until (($state -and $state.pages -ge 1) -or (Get-Date) -gt $deadline)
}

function Stop-Window {
    if ($script:window -and -not $script:window.HasExited) {
        Stop-Process -Id $script:window.Id -Force
        $script:window.WaitForExit(10000) | Out-Null
    }
    $script:window = $null
}

# The widgets on screen.
function Get-Widgets { return (Ui inspect | ConvertFrom-Json).widgets }

# The printer list arrives on a worker and can take a while, so what depends on it comes later.
function Test-OnScreen([string]$Case, [string]$Text) {
    $start = $Text.Substring(0, [Math]::Min(60, $Text.Length))
    $deadline = (Get-Date).AddSeconds(60)
    do {
        $found = Get-Widgets | Where-Object { $_.label -eq $Text -or $_.value -eq $Text -or $_.value -like "$start*" }
        if ($found) { return }
        Start-Sleep -Milliseconds 500
    } while ((Get-Date) -lt $deadline)
    Fail $Case "the window doesn't say: $Text"
}

# Click the last widget with this label: what a dialog draws comes after the toolbar under it.
function Invoke-Click([string]$Label) {
    $target = Get-Widgets | Where-Object { $_.label -eq $Label -and $_.enabled } | Select-Object -Last 1
    if (-not $target) { throw "nothing to click labelled $Label" }
    Ui click "id=$($target.id)" | Out-Null
    Start-Sleep -Milliseconds 400
}

# Open a menu by its current value and pick an entry, waiting for the menu to draw it.
function Select-Menu([string]$Shown, [string]$Entry) {
    $deadline = (Get-Date).AddSeconds(30)
    do {
        $menu = Get-Widgets | Where-Object { $_.value -eq $Shown -and $_.role -notin 'Label', 'TextRun' } | Select-Object -Last 1
        if (-not $menu) { throw "no menu showing $Shown" }
        Ui click "id=$($menu.id)" | Out-Null
        Start-Sleep -Seconds 1
        if (Get-Widgets | Where-Object { $_.label -eq $Entry -and $_.enabled }) {
            Invoke-Click $Entry
            return
        }
    } while ((Get-Date) -lt $deadline)
    throw "the menu showing $Shown never offered $Entry"
}

function Open-PrintDialog {
    Ui command id=print.dialog | Out-Null
    $deadline = (Get-Date).AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 300
        $looking = Get-Widgets | Where-Object { $_.value -eq 'Looking for printers…' }
    } while ($looking -and (Get-Date) -lt $deadline)
}

function Test-WindowCase([string]$Name, [string]$File, [hashtable]$Expect, [string]$Reason, [string]$Button, [object[]]$Checks, [string]$Orientation) {
    Invoke-Case "window: $Name" {
        Start-Window $File
        try {
            Open-PrintDialog
            Test-OnScreen "window: $Name" $Reason
            $held = Invoke-HeldJob -Expect $Expect -Send { Invoke-Click $Button }
            foreach ($job in $held) { [void](Test-Job "window: $Name" $job $Checks $Orientation) }
        } finally { Stop-Window }
    }
}

function Invoke-WindowCases {
    Write-Rules @()
    $quiet = @{ $canonC = 0; $canonD = 0; $office = 0 }
    Test-WindowCase '18x24 portrait' $pdf.cPortrait @{ $canonC = 1 } "18 x 24 detected: $canonC" 'Print' @($rollC, $archC) 'Portrait'
    Test-WindowCase '18x24 landscape' $pdf.cLandscape @{ $canonC = 1 } "18 x 24 detected: $canonC" 'Print' @($rollC, $archC) 'Landscape'
    Test-WindowCase '24x36' $pdf.dLandscape @{ $canonD = 1 } "24 x 36 detected: $canonD" 'Print' @($rollD, $archD) 'Landscape'
    Test-WindowCase 'Letter' $pdf.letter @{ $office = 1 } "Letter detected: $office" 'Print' @(, $letterPaper) 'Portrait'

    Invoke-Case 'window: mixed sizes as separate jobs' {
        Start-Window $pdf.mixed
        try {
            Open-PrintDialog
            Test-OnScreen 'window: mixed' 'Mixed page sizes: 2 pages 18 x 24, 1 page Letter, 1 page 24 x 36.'
            $held = Invoke-HeldJob -Expect @{ $canonC = 1; $canonD = 1; $office = 1 } -Send { Invoke-Click 'Send 3 jobs' }
            foreach ($job in $held) {
                switch ($job.printer) {
                    $canonC { [void](Test-Job 'window: mixed' $job @($rollC, $archC) $null) }
                    $canonD { [void](Test-Job 'window: mixed' $job @($rollD, $archD) $null) }
                    $office { [void](Test-Job 'window: mixed' $job @(, $letterPaper) $null) }
                }
            }
        } finally { Stop-Window }
    }

    Invoke-Case 'window: no matching rule holds Print' {
        Start-Window $pdf.noRule
        try {
            Open-PrintDialog
            Test-OnScreen 'window: no rule' '22 x 34 detected, and no printer rule covers it. Choose a printer.'
            [void](Invoke-HeldJob -Expect $quiet -Send {
                    $dialogPrint = Get-Widgets | Where-Object { $_.label -eq 'Print' -and $_.role -eq 'Button' } | Select-Object -Last 1
                    if ($dialogPrint.enabled) { Fail 'window: no rule' 'Print is enabled' }
                })
        } finally { Stop-Window }
    }

    Invoke-Case 'window: a rule for a missing printer holds Print' {
        Write-Rules @(@{ paper = 'ARCH C'; printer = 'No Such Printer' })
        Start-Window $pdf.cPortrait
        try {
            Open-PrintDialog
            Test-OnScreen 'window: missing' "18 x 24 detected, but the printer rule names No Such Printer, which isn't installed. Choose a printer."
            [void](Invoke-HeldJob -Expect $quiet -Send {
                    $dialogPrint = Get-Widgets | Where-Object { $_.label -eq 'Print' -and $_.role -eq 'Button' } | Select-Object -Last 1
                    if ($dialogPrint.enabled) { Fail 'window: missing' 'Print is enabled' }
                })
        } finally {
            Stop-Window
            Write-Rules @()
        }
    }

    Invoke-Case 'window: a manual override warns, then goes where it was sent' {
        Start-Window $pdf.cPortrait
        try {
            Open-PrintDialog
            Test-OnScreen 'window: menu' "18 x 24 detected: $canonC"
            Select-Menu $canonC $canonD
            Test-OnScreen 'window: override' "These pages are 18 x 24, which the printer rules send to $canonC. You have selected $canonD, ARCH C (18 x 24 in)."
            $held = Invoke-HeldJob -Expect @{ $canonD = 1; $canonC = 0 } -Send { Invoke-Click 'Print anyway' }
            [void](Test-Job 'window: override' $held[0] @($rollD, $archC) 'Portrait')
        } finally { Stop-Window }
    }

    Invoke-Case 'window: the one-click fix sends it to the rule''s printer' {
        Start-Window $pdf.cPortrait
        try {
            Open-PrintDialog
            Test-OnScreen 'window: menu' "18 x 24 detected: $canonC"
            Select-Menu $canonC $canonD
            Invoke-Click "Use $canonC, ARCH C (18 x 24 in)"
            $held = Invoke-HeldJob -Expect @{ $canonC = 1; $canonD = 0 } -Send { Invoke-Click 'Print' }
            [void](Test-Job 'window: fix' $held[0] @($rollC, $archC) 'Portrait')
        } finally { Stop-Window }
    }

    Invoke-Case 'window: the last-used paper differing from the PDF is ignored' {
        Start-Window $pdf.letter
        try {
            Open-PrintDialog
            # Tabloid picked by hand for a job that never prints: now the last paper used.
            Select-Menu 'US Letter' 'Tabloid'
            Invoke-Click 'Cancel'
            Ui open "path=$($pdf.cPortrait)" | Out-Null
            Start-Sleep -Seconds 2
            Open-PrintDialog
            Test-OnScreen 'window: last paper' "18 x 24 detected: $canonC"
            $held = Invoke-HeldJob -Expect @{ $canonC = 1 } -Send { Invoke-Click 'Print' }
            [void](Test-Job 'window: last paper' $held[0] @($rollC, $archC) 'Portrait')
        } finally { Stop-Window }
    }
}

try {
    if ($Only -in 'all', 'cli') { Invoke-CliCases }
    if ($Only -in 'all', 'window') { Invoke-WindowCases }
} finally {
    Stop-Window
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

foreach ($printer in $canonC, $canonD, $office) {
    $queue = Get-AtlasQueue $printer
    if ($queue.IsPaused -or $queue.NumberOfJobs -ne 0) { $failures.Add("$printer ended paused=$($queue.IsPaused) with $($queue.NumberOfJobs) job(s)") }
}
Write-Host ''
Write-Host "$passed case(s) passed, $($failures.Count) failure(s)."
foreach ($failure in $failures) { Write-Host "  $failure" -ForegroundColor Red }
if ($failures.Count -ne 0) { exit 1 }
