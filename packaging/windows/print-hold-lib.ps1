# Atlas fork: test what a print job really carries, without using paper.
#
# Dot-source this file. Invoke-HeldJob pauses Windows queues, checks that each reports Paused, runs
# a script block that sends jobs, reads each held job's DEVMODE with GetJob level 2, converts it to
# a print ticket through the driver, then deletes the jobs and resumes the queues, also when the
# test fails. Win32_PrintJob reports UNKNOWN paper for these jobs, so it isn't used.
#
# A queue with jobs already in it, or one that won't report Paused, stops the test before anything
# is sent. A job that can't be deleted leaves its queue paused, so it can't print.

Add-Type -AssemblyName System.Printing
Add-Type -AssemblyName ReachFramework

if (-not ('AtlasHeldJob' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

public static class AtlasHeldJob {
    [DllImport("winspool.drv", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool OpenPrinterW(string name, out IntPtr handle, IntPtr defaults);
    [DllImport("winspool.drv", SetLastError = true)]
    static extern bool ClosePrinter(IntPtr handle);
    [DllImport("winspool.drv", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool GetJobW(IntPtr handle, uint id, uint level, IntPtr buffer, uint size, out uint needed);
    [DllImport("winspool.drv", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool SetJobW(IntPtr handle, uint id, uint level, IntPtr info, uint command);

    const uint JOB_CONTROL_DELETE = 5;
    // JOB_INFO_2W: the job id, then pointers (aligned), so pDevMode is the tenth pointer.
    const int DevModePointerIndex = 9;

    static IntPtr Open(string printer) {
        IntPtr handle;
        if (!OpenPrinterW(printer, out handle, IntPtr.Zero)) throw new Win32Exception();
        return handle;
    }

    /// The job's DEVMODE: the public fields and the driver's private data, as the spooler holds them.
    public static byte[] DevMode(string printer, uint id) {
        IntPtr handle = Open(printer);
        try {
            uint needed;
            GetJobW(handle, id, 2, IntPtr.Zero, 0, out needed);
            if (needed == 0) throw new Win32Exception();
            IntPtr buffer = Marshal.AllocHGlobal((int)needed);
            try {
                if (!GetJobW(handle, id, 2, buffer, needed, out needed)) throw new Win32Exception();
                IntPtr devmode = Marshal.ReadIntPtr(buffer, IntPtr.Size + DevModePointerIndex * IntPtr.Size);
                if (devmode == IntPtr.Zero) throw new InvalidOperationException("the held job has no DEVMODE");
                int size = Marshal.ReadInt16(devmode, 68) + Marshal.ReadInt16(devmode, 70);
                byte[] bytes = new byte[size];
                Marshal.Copy(devmode, bytes, 0, size);
                return bytes;
            } finally { Marshal.FreeHGlobal(buffer); }
        } finally { ClosePrinter(handle); }
    }

    public static void Delete(string printer, uint id) {
        IntPtr handle = Open(printer);
        try {
            if (!SetJobW(handle, id, 0, IntPtr.Zero, JOB_CONTROL_DELETE)) throw new Win32Exception();
        } finally { ClosePrinter(handle); }
    }
}
'@
}

function Get-AtlasQueue([string]$Printer) {
    # Pausing needs a queue opened to manage it; the default open only reads.
    $server = New-Object System.Printing.PrintServer
    return New-Object System.Printing.PrintQueue($server, $Printer, [System.Printing.PrintSystemDesiredAccess]::AdministratePrinter)
}

# The public DEVMODE fields a held job carries.
function Read-DevModeFields([byte[]]$DevMode) {
    $form = [Text.Encoding]::Unicode.GetString($DevMode, 102, 64).Split([char]0)[0]
    return [ordered]@{
        orientation = @{ 1 = 'Portrait'; 2 = 'Landscape' }[[int][BitConverter]::ToInt16($DevMode, 76)]
        paper_code  = [BitConverter]::ToInt16($DevMode, 78)
        length_mm   = [BitConverter]::ToInt16($DevMode, 80) / 10.0
        width_mm    = [BitConverter]::ToInt16($DevMode, 82) / 10.0
        copies      = [BitConverter]::ToInt16($DevMode, 86)
        source      = [BitConverter]::ToInt16($DevMode, 88)
        form        = $form
    }
}

# The DEVMODE converted to a print ticket by the job's driver, as feature name -> option name
# (local names, so psk:JobInputBin = ns0000:RollPaper2 reads JobInputBin = RollPaper2).
function Convert-DevModeToFeatures([string]$Printer, [byte[]]$DevMode) {
    $version = [System.Printing.Interop.PrintTicketConverter]::MaxPrintSchemaVersion
    $converter = New-Object System.Printing.Interop.PrintTicketConverter($Printer, $version)
    try {
        $scope = [System.Printing.PrintTicketScope]::JobScope
        $ticket = $converter.ConvertDevModeToPrintTicket($DevMode, $scope)
    } finally { $converter.Dispose() }
    return Read-TicketFeatures $ticket.GetXmlStream().ToArray()
}

function Read-TicketFeatures([byte[]]$Xml) {
    $document = New-Object Xml.XmlDocument
    $document.Load((New-Object IO.MemoryStream(, $Xml)))
    $names = New-Object Xml.XmlNamespaceManager $document.NameTable
    $names.AddNamespace('psf', 'http://schemas.microsoft.com/windows/2003/08/printing/printschemaframework')
    $features = [ordered]@{}
    foreach ($feature in $document.SelectNodes('/psf:PrintTicket/psf:Feature', $names)) {
        $option = $feature.SelectSingleNode('psf:Option', $names)
        if ($null -ne $option) { $features[$feature.name.Split(':')[-1]] = $option.name.Split(':')[-1] }
    }
    return $features
}

# Delete $Queue's jobs and resume it. Returns $false if a job stayed, leaving the queue paused.
function Clear-AtlasQueue($Queue, [string]$Printer) {
    $Queue.Refresh()
    foreach ($job in @($Queue.GetPrintJobInfoCollection())) {
        try { [AtlasHeldJob]::Delete($Printer, [uint32]$job.JobIdentifier) }
        catch { Write-Warning "could not delete job $($job.JobIdentifier) on ${Printer}: $_" }
    }
    $deadline = (Get-Date).AddSeconds(60)
    do {
        Start-Sleep -Milliseconds 250
        $Queue.Refresh()
    } until ($Queue.NumberOfJobs -eq 0 -or (Get-Date) -gt $deadline)
    if ($Queue.NumberOfJobs -ne 0) {
        Write-Warning "$Printer still holds a job, so it stays PAUSED. Clear the queue by hand, then resume it."
        return $false
    }
    $Queue.Resume()
    $Queue.Refresh()
    if ($Queue.IsPaused) { Write-Warning "$Printer did not resume"; return $false }
    return $true
}

# Pause the queues named in $Expect (printer name -> jobs expected on it), run $Send, and return
# what each job holds. A queue expecting none is watched for $QuietSeconds. Always cleans up.
function Invoke-HeldJob {
    param(
        [Parameter(Mandatory)][hashtable]$Expect,
        [Parameter(Mandatory)][scriptblock]$Send,
        [int]$TimeoutSeconds = 180,
        [int]$QuietSeconds = 6
    )
    $queues = @{}
    foreach ($printer in $Expect.Keys) {
        $queue = Get-AtlasQueue $printer
        if ($queue.NumberOfJobs -ne 0) { throw "$printer has jobs in its queue already; nothing was sent" }
        if ($queue.IsPaused) { throw "$printer was already paused; nothing was sent" }
        $queues[$printer] = $queue
    }
    $defaultBefore = (Get-CimInstance Win32_Printer -Filter 'Default=True').Name
    $held = @()
    $clean = $true
    $paused = @()
    try {
        foreach ($printer in $Expect.Keys) {
            $queues[$printer].Pause()
            $paused += $printer
            $queues[$printer].Refresh()
            if (-not $queues[$printer].IsPaused) { throw "$printer did not report Paused; nothing was sent" }
        }
        & $Send
        $wanted = ($Expect.Values | Measure-Object -Sum).Sum
        $deadline = (Get-Date).AddSeconds($(if ($wanted -gt 0) { $TimeoutSeconds } else { $QuietSeconds }))
        do {
            Start-Sleep -Milliseconds 500
            $settled = $true
            foreach ($printer in $Expect.Keys) {
                $queues[$printer].Refresh()
                $listed = @($queues[$printer].GetPrintJobInfoCollection())
                if ($listed.Count -lt $Expect[$printer] -or ($listed | Where-Object { $_.IsSpooling })) { $settled = $false }
            }
        } until (($wanted -gt 0 -and $settled) -or (Get-Date) -gt $deadline)
        foreach ($printer in $Expect.Keys) {
            $queues[$printer].Refresh()
            $listed = @($queues[$printer].GetPrintJobInfoCollection())
            if ($listed.Count -ne $Expect[$printer]) { throw "$printer holds $($listed.Count) job(s), expected $($Expect[$printer])" }
            foreach ($job in $listed) {
                $id = [uint32]$job.JobIdentifier
                $devmode = [AtlasHeldJob]::DevMode($printer, $id)
                $held += [pscustomobject]@{
                    printer = $printer
                    id      = $id
                    name    = $job.Name
                    pages   = $job.NumberOfPages
                    devmode = Read-DevModeFields $devmode
                    ticket  = Convert-DevModeToFeatures $printer $devmode
                }
            }
        }
    } finally {
        foreach ($printer in $paused) {
            if (-not (Clear-AtlasQueue $queues[$printer] $printer)) { $clean = $false }
        }
    }
    $defaultAfter = (Get-CimInstance Win32_Printer -Filter 'Default=True').Name
    if ($defaultAfter -ne $defaultBefore) { throw "the Windows default printer changed from $defaultBefore to $defaultAfter" }
    if (-not $clean) { throw 'a queue was left paused' }
    return $held
}

# A synthetic drawing. $Pages: @{ w; h; crop = @(x0,y0,x1,y1) or $null; rotate = 0; label = 'text' },
# in points. Thin lines: an inset border, inch ticks along the bottom and left edges, an arrow at
# the top and the label, so a printed sheet can be measured.
function New-AtlasTestPdf([string]$Path, [object[]]$Pages) {
    $objects = New-Object Collections.Generic.List[string]
    $objects.Add('<< /Type /Catalog /Pages 2 0 R >>')
    $kids = @()
    for ($i = 0; $i -lt $Pages.Count; $i++) { $kids += "$(3 + 2 * $i) 0 R" }
    $objects.Add("<< /Type /Pages /Kids [$($kids -join ' ')] /Count $($Pages.Count) >>")
    $font = 3 + 2 * $Pages.Count
    for ($i = 0; $i -lt $Pages.Count; $i++) {
        $p = $Pages[$i]
        $x0 = 0; $y0 = 0; $w = $p.w; $h = $p.h
        $extra = ''
        if ($p.crop) {
            $extra += " /CropBox [$($p.crop -join ' ')]"
            $x0 = $p.crop[0]; $y0 = $p.crop[1]; $w = $p.crop[2] - $p.crop[0]; $h = $p.crop[3] - $p.crop[1]
        }
        if ($p.rotate) { $extra += " /Rotate $($p.rotate)" }
        $objects.Add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 $($p.w) $($p.h)]$extra /Resources << /Font << /F1 $font 0 R >> >> /Contents $(4 + 2 * $i) 0 R >>")
        $c = New-Object Text.StringBuilder
        [void]$c.AppendLine('0.5 w')
        [void]$c.AppendLine("$($x0 + 36) $($y0 + 36) $($w - 72) $($h - 72) re S")
        for ($x = 72; $x -lt $w; $x += 72) { [void]$c.AppendLine("$($x0 + $x) $($y0 + 36) m $($x0 + $x) $($y0 + 48) l S") }
        for ($y = 72; $y -lt $h; $y += 72) { [void]$c.AppendLine("$($x0 + 36) $($y0 + $y) m $($x0 + 48) $($y0 + $y) l S") }
        $mx = $x0 + $w / 2; $top = $y0 + $h - 36
        [void]$c.AppendLine("$mx $($top - 72) m $mx $top l S $($mx - 12) $($top - 18) m $mx $top l $($mx + 12) $($top - 18) l S")
        [void]$c.AppendLine("BT /F1 28 Tf $($x0 + 72) $($y0 + 72) Td ($($p.label)) Tj ET")
        $content = $c.ToString()
        $objects.Add("<< /Length $($content.Length) >>`nstream`n$content`nendstream")
    }
    $objects.Add('<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>')
    $out = New-Object Text.StringBuilder
    [void]$out.Append("%PDF-1.7`n")
    $offsets = @()
    for ($i = 0; $i -lt $objects.Count; $i++) {
        $offsets += $out.Length
        [void]$out.Append("$($i + 1) 0 obj`n$($objects[$i])`nendobj`n")
    }
    $xref = $out.Length
    [void]$out.Append("xref`n0 $($objects.Count + 1)`n0000000000 65535 f `n")
    foreach ($offset in $offsets) { [void]$out.Append(('{0:D10} 00000 n ' -f $offset) + "`n") }
    [void]$out.Append("trailer << /Size $($objects.Count + 1) /Root 1 0 R >>`nstartxref`n$xref`n%%EOF`n")
    [IO.File]::WriteAllBytes($Path, [Text.Encoding]::ASCII.GetBytes($out.ToString()))
}
