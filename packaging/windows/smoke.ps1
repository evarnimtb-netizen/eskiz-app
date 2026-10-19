# Windows smoke test: runs a release eskiz.exe the way a user would and records what it saw.
#
#   1. eskiz --version, --selftest (which also writes %TEMP%\selftest.procreate), --diagnose and
#      --diagnose --window (GPU adapter and backend, scale factor of a real window).
#   2. Opens the real window on selftest.procreate, takes screenshots, paints one stroke with real
#      mouse input, presses Ctrl+Z, and counts how many pixels each step changed.
#   3. Idles for 30 s with a stroke every 10 s, closes the window, and checks that the app never exited
#      on its own, never turned "Not Responding" (IsHungAppWindow) and never left a stalled.marker.
#   4. Does 2 and 3 three times: the default start, `--renderer dx12` (DX12 with the GPU compositor)
#      and `--renderer software` (the same adapter, CPU compositor).
#   5. Opens the document with `--renderer gl`. Without a usable OpenGL (GitHub's VM) the step says
#      SKIP with the reason; it does not count the software fallback as an OpenGL pass.
#
# Output folder (default eskiz\target\windows-smoke): numbered *.png screenshots (whole screen and
# the window alone), one log per command, eskiz-diagnose.md and smoke-report.md. In GitHub Actions
# the report is also added to the job summary.
#
#   pwsh -File packaging\windows\smoke.ps1 [-Exe path\to\eskiz.exe] [-OutDir dir] [-StrictInput]
#
# Exit 1 when a command fails, the window never appears, or the app exits before it is closed.
# A stroke or undo that changes no pixels is only a warning unless -StrictInput is given, because a
# runner's desktop may not deliver synthetic mouse and keyboard input.
[CmdletBinding()]
param(
    [string]$Exe,
    [string]$OutDir,
    [switch]$StrictInput,
    # Seconds to wait for each step of the window run.
    [int]$WindowTimeout = 90
)

$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$eskizDir = (Resolve-Path (Join-Path $here '..\..')).Path
if (-not $Exe) { $Exe = Join-Path $eskizDir 'target\release\eskiz.exe' }
if (-not $OutDir) { $OutDir = Join-Path $eskizDir 'target\windows-smoke' }
if (-not (Test-Path $Exe)) { throw "missing $Exe (build it first: cargo build --release -p eskiz-app)" }
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path

# Keep the run's documents, brushes and fonts out of the user's own folders.
$data = Join-Path $OutDir 'data'
New-Item -ItemType Directory -Force -Path $data | Out-Null
$env:ESKIZ_LIBRARY_DIR = Join-Path $data 'library'
$env:ESKIZ_BRUSH_DIR = Join-Path $data 'brushes'
$env:ESKIZ_FONT_DIR = Join-Path $data 'fonts'
$env:RUST_BACKTRACE = '1'
# The start-up log, crash marker and stall marker of the runs go here, not to %LOCALAPPDATA%.
$script:logDir = Join-Path $OutDir 'logs'
New-Item -ItemType Directory -Force -Path $script:logDir | Out-Null
$env:ESKIZ_LOG_DIR = $script:logDir

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class SmokeWin {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool IsHungAppWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int cmd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hwnd, ref POINT point);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code, uint mapType);

    /// A key press or release with its hardware scan code. The window toolkit identifies a key by
    /// its scan code; an event without one (scan 0) reaches the app as an unknown key.
    public static void Key(byte vk, bool up) {
        byte scan = (byte)MapVirtualKey(vk, 0);
        keybd_event(vk, scan, up ? 2u : 0u, UIntPtr.Zero);
    }

    /// Pixels (BGRA, 4 bytes each) whose largest colour channel differs by more than threshold.
    /// -1 when the two images are not the same size.
    public static int Changed(byte[] a, byte[] b, int threshold) {
        if (a == null || b == null || a.Length != b.Length) return -1;
        int n = 0;
        for (int i = 0; i + 3 < a.Length; i += 4) {
            int d = Math.Max(Math.Abs(a[i] - b[i]), Math.Max(Math.Abs(a[i + 1] - b[i + 1]), Math.Abs(a[i + 2] - b[i + 2])));
            if (d > threshold) n++;
        }
        return n;
    }

    /// Pixels that differ from the first pixel: 0 means the image is one flat colour (a black capture).
    public static int NotLikeFirst(byte[] a) {
        int n = 0;
        for (int i = 4; i + 3 < a.Length; i += 4) {
            if (a[i] != a[0] || a[i + 1] != a[1] || a[i + 2] != a[2]) n++;
        }
        return n;
    }
}
'@
# Real pixel coordinates for the cursor and the screen capture, whatever the display scale.
[void][SmokeWin]::SetProcessDPIAware()

# ------------------------------------------------------------------------------------- report

$rows = New-Object System.Collections.Generic.List[string]
$failures = New-Object System.Collections.Generic.List[string]
$warnings = New-Object System.Collections.Generic.List[string]

function Add-Row([string]$step, [string]$result, [string]$detail) {
    $rows.Add("| $step | $result | $($detail -replace '\|', '/' -replace "`r?`n", ' ') |")
    Write-Host "   $step : $result $detail"
}
function Fail([string]$step, [string]$detail) {
    $failures.Add("${step}: $detail")
    Add-Row $step 'FAIL' $detail
}
function Warn([string]$step, [string]$detail) {
    $warnings.Add("${step}: $detail")
    Add-Row $step 'WARN' $detail
    if ($env:GITHUB_ACTIONS) { Write-Host "::warning title=Windows smoke ($step)::$detail" }
}
# Synthetic input that had no visible effect: a failure only with -StrictInput.
function Input-Problem([string]$step, [string]$detail) {
    if ($StrictInput) { Fail $step $detail } else { Warn $step $detail }
}

function Tail([string]$path, [int]$lines = 15) {
    if (Test-Path $path) { (Get-Content -Path $path -Tail $lines) -join ' / ' } else { '' }
}

# Start eskiz.exe with stdout and stderr in <name>.log and <name>.err.log, in the output folder.
function Start-Eskiz([string]$name, [string[]]$arguments) {
    $params = @{
        FilePath               = $Exe
        WorkingDirectory       = $OutDir
        RedirectStandardOutput = (Join-Path $OutDir "$name.log")
        RedirectStandardError  = (Join-Path $OutDir "$name.err.log")
        PassThru               = $true
    }
    if ($arguments) { $params.ArgumentList = $arguments }
    $p = Start-Process @params
    # Without touching Handle now, ExitCode is empty after the process has exited.
    $null = $p.Handle
    $p
}

# Run a command to completion and record it. Returns true when it exited with 0.
function Invoke-Eskiz([string]$name, [string[]]$arguments, [int]$timeout) {
    Write-Host "== eskiz $($arguments -join ' ')"
    $p = Start-Eskiz $name $arguments
    if (-not $p.WaitForExit($timeout * 1000)) {
        $p.Kill()
        Fail "eskiz $($arguments -join ' ')" "still running after $timeout s; killed. $(Tail (Join-Path $OutDir "$name.err.log"))"
        return $false
    }
    $p.WaitForExit()
    $out = Join-Path $OutDir "$name.log"
    Get-Content -Path $out -ErrorAction SilentlyContinue | Write-Host
    if ($p.ExitCode -ne 0) {
        Fail "eskiz $($arguments -join ' ')" "exit $($p.ExitCode). $(Tail (Join-Path $OutDir "$name.err.log"))"
        return $false
    }
    $first = (Get-Content -Path $out -TotalCount 1 -ErrorAction SilentlyContinue)
    Add-Row "eskiz $($arguments -join ' ')" 'PASS' $first
    $true
}

# A window that stops answering is "Not Responding" (IsHungAppWindow, Process.Responding). Windows
# takes about 5 s to decide, so a check is a pair of looks a few seconds apart; one hit is a failure.
# The app's own watchdog also leaves `STALL:` in startup.log (ESKIZ_LOG_DIR below).
function Test-Hung($proc, [IntPtr]$hwnd, [string]$step) {
    if ($proc.HasExited) { return $false }
    $proc.Refresh()
    $hung = [SmokeWin]::IsHungAppWindow($hwnd) -or (-not $proc.Responding)
    if ($hung) {
        Fail "hung window ($step)" 'the window is "Not Responding" (IsHungAppWindow). The app froze; see startup.log STALL line in the artifact.'
    }
    $hung
}

function Save-StartupLog([string]$name) {
    $log = Join-Path $script:logDir 'startup.log'
    if (Test-Path $log) { Copy-Item -Force $log (Join-Path $OutDir "$name-startup.log") }
    $marker = Join-Path $script:logDir 'stalled.marker'
    if (Test-Path $marker) {
        Fail "stall marker ($name)" "the app's watchdog saw a stall: $((Get-Content $marker) -join ' ')"
    }
}

# ------------------------------------------------------------------------------------- capture

function Get-Bytes([System.Drawing.Bitmap]$bmp) {
    $rect = New-Object System.Drawing.Rectangle 0, 0, $bmp.Width, $bmp.Height
    $lock = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    try {
        $bytes = New-Object byte[] ($lock.Stride * $bmp.Height)
        [System.Runtime.InteropServices.Marshal]::Copy($lock.Scan0, $bytes, 0, $bytes.Length)
        , $bytes
    }
    finally { $bmp.UnlockBits($lock) }
}

# The whole screen as the desktop compositor shows it. Saved as <name>-screen.png.
function Save-Screen([string]$name) {
    $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    try {
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
        $g.Dispose()
        $bmp.Save((Join-Path $OutDir "$name-screen.png"), [System.Drawing.Imaging.ImageFormat]::Png)
        , (Get-Bytes $bmp)
    }
    finally { $bmp.Dispose() }
}

# The window alone, drawn by PrintWindow with PW_RENDERFULLCONTENT (2) so that GPU-presented
# content is included even when another window covers it. Saved as <name>-window.png.
function Save-Window([IntPtr]$hwnd, [string]$name) {
    $r = New-Object SmokeWin+RECT
    if (-not [SmokeWin]::GetWindowRect($hwnd, [ref]$r)) { return , $null }
    $w = $r.Right - $r.Left
    $h = $r.Bottom - $r.Top
    if ($w -le 0 -or $h -le 0) { return , $null }
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    try {
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $hdc = $g.GetHdc()
        [void][SmokeWin]::PrintWindow($hwnd, $hdc, 2)
        $g.ReleaseHdc($hdc)
        $g.Dispose()
        $bmp.Save((Join-Path $OutDir "$name-window.png"), [System.Drawing.Imaging.ImageFormat]::Png)
        , (Get-Bytes $bmp)
    }
    finally { $bmp.Dispose() }
}

# Both captures of one moment.
function Save-Shot([IntPtr]$hwnd, [string]$name) {
    Write-Host "   screenshot $name"
    @{ Screen = (Save-Screen $name); Window = (Save-Window $hwnd $name) }
}

function Describe-Change($before, $after) {
    $s = [SmokeWin]::Changed($before.Screen, $after.Screen, 24)
    $w = if ($before.Window -and $after.Window) { [SmokeWin]::Changed($before.Window, $after.Window, 24) } else { -1 }
    @{ Screen = $s; Window = $w; Text = "screen $s px, window $(if ($w -ge 0) { "$w px" } else { 'n/a (size changed)' })" }
}

# ------------------------------------------------------------------------------------- input

function Get-ClientPoint([IntPtr]$hwnd, [double]$fx, [double]$fy) {
    $c = New-Object SmokeWin+RECT
    [void][SmokeWin]::GetClientRect($hwnd, [ref]$c)
    $p = New-Object SmokeWin+POINT
    $p.X = [int]($c.Right * $fx)
    $p.Y = [int]($c.Bottom * $fy)
    [void][SmokeWin]::ClientToScreen($hwnd, [ref]$p)
    $p
}

function Focus-Window([IntPtr]$hwnd) {
    if ([SmokeWin]::GetForegroundWindow() -eq $hwnd) { return }
    # Windows only lets a process take the foreground right after input; a tap of Alt counts. Only
    # while another window has the focus: Alt released in our window would open its system menu.
    [SmokeWin]::Key(0x12, $false)
    [SmokeWin]::Key(0x12, $true)
    [void][SmokeWin]::SetForegroundWindow($hwnd)
    Start-Sleep -Milliseconds 500
}

# One left-button drag across the middle of the client area, a gentle wave, about 1 s long.
function Invoke-Stroke([IntPtr]$hwnd, [double]$y = 0.50) {
    $steps = 60
    $start = Get-ClientPoint $hwnd 0.30 $y
    [void][SmokeWin]::SetCursorPos($start.X, $start.Y)
    Start-Sleep -Milliseconds 200
    [SmokeWin]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero) # left down
    for ($i = 1; $i -le $steps; $i++) {
        $t = $i / $steps
        $p = Get-ClientPoint $hwnd (0.30 + 0.40 * $t) ($y + 0.08 * [Math]::Sin($t * 2 * [Math]::PI))
        [void][SmokeWin]::SetCursorPos($p.X, $p.Y)
        Start-Sleep -Milliseconds 16
    }
    [SmokeWin]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero) # left up
    # The cursor stays at the end of the stroke, so a brush outline there is in the undo shot too.
}

function Invoke-Undo {
    # Ctrl+Z with real scan codes and short holds, like a keyboard (see SmokeWin.Key).
    [SmokeWin]::Key(0x11, $false) # Ctrl down
    Start-Sleep -Milliseconds 80
    [SmokeWin]::Key(0x5A, $false) # Z down
    Start-Sleep -Milliseconds 80
    [SmokeWin]::Key(0x5A, $true)
    Start-Sleep -Milliseconds 80
    [SmokeWin]::Key(0x11, $true)
}

# ------------------------------------------------------------------------------------- run

$sample = Join-Path ([System.IO.Path]::GetTempPath()) 'selftest.procreate'

# Forget the previous run's start-up log, markers and remembered skips, so every window run starts
# the way a first start does and its log is its own.
function Reset-Logs {
    foreach ($f in 'startup.log', 'starting.marker', 'stalled.marker', 'renderer-skip.txt', 'graphics.pref') {
        Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $script:logDir $f)
    }
}

# The first log line matching a pattern, or ''.
function Find-LogLine([string]$pattern) {
    $logFile = Join-Path $script:logDir 'startup.log'
    if (-not (Test-Path $logFile)) { return '' }
    $hit = Select-String -Path $logFile -Pattern $pattern | Select-Object -First 1
    if ($hit) { $hit.Line } else { '' }
}

# Open the sample document in a real window with the given renderer arguments and use it: a stroke,
# Ctrl+Z, then 30 s of idling with a stroke every 10 s (the frozen DX12 window stopped answering while
# idle), then close. $label names the run in the report and the screenshots; $expect is a regex the
# log's "renderer:" line must match so a silent fallback to another renderer is not taken for a pass.
function Invoke-WindowRun([string]$label, [string[]]$extra, [string]$expect) {
    Write-Host "== window run '$label' ($($extra -join ' '))"
    if (-not (Test-Path $sample)) { Fail "window ($label)" "no $sample (the self-test writes it)"; return }
    Reset-Logs
    $app = Start-Eskiz "window-$label" (@($extra) + @("`"$sample`""))
    try {
        $deadline = (Get-Date).AddSeconds($WindowTimeout)
        while ((Get-Date) -lt $deadline -and -not $app.HasExited) {
            $app.Refresh()
            if ($app.MainWindowHandle -ne [IntPtr]::Zero) { break }
            Start-Sleep -Milliseconds 250
        }
        $hwnd = $app.MainWindowHandle
        if ($app.HasExited) {
            Fail "window opens ($label)" "the app exited with $($app.ExitCode) before its window appeared. $(Tail (Join-Path $OutDir "window-$label.err.log"))"
            return
        }
        if ($hwnd -eq [IntPtr]::Zero) {
            [void](Save-Screen "$label-00-no-window")
            Fail "window opens ($label)" "no window after $WindowTimeout s"
            return
        }
        Add-Row "window opens ($label)" 'PASS' "title '$($app.MainWindowTitle)'"
        # Let it draw a few frames on a slow software renderer.
        Start-Sleep -Seconds 5
        $chosen = Find-LogLine 'renderer: '
        $comp = Find-LogLine 'compositor: '
        Add-Row "renderer ($label)" 'info' "$chosen $comp"
        if ($expect -and $chosen -notmatch $expect) {
            Warn "renderer ($label)" "expected '$expect' in the log's renderer line, got: $chosen"
        }
        if (-not (Test-Hung $app $hwnd "$label after opening")) { Add-Row "responsive after opening ($label)" 'PASS' 'not hung' }
        $opened = Save-Shot $hwnd "$label-01-opened"
        if ([SmokeWin]::NotLikeFirst($opened.Screen) -eq 0) { Warn "screen capture ($label)" 'the screenshot is one flat colour' }

        [void][SmokeWin]::ShowWindow($hwnd, 3) # SW_MAXIMIZE: the same layout on every run
        Focus-Window $hwnd
        Start-Sleep -Seconds 3
        $before = Save-Shot $hwnd "$label-02-maximized"
        if ($before.Window -and [SmokeWin]::NotLikeFirst($before.Window) -eq 0) {
            Warn "window capture ($label)" 'PrintWindow returned one flat colour (GPU content not captured)'
        }

        Write-Host "== stroke"
        Invoke-Stroke $hwnd 0.50
        Start-Sleep -Seconds 2
        $stroke = Save-Shot $hwnd "$label-03-stroke"
        [void](Test-Hung $app $hwnd "$label after the stroke")
        $change = Describe-Change $before $stroke
        if ($app.HasExited) { Fail "stroke ($label)" "the app exited with $($app.ExitCode)" }
        elseif ($change.Screen -gt 200 -or $change.Window -gt 200) { Add-Row "stroke ($label)" 'PASS' "changed: $($change.Text)" }
        else { Input-Problem "stroke ($label)" "the drag changed almost nothing: $($change.Text)" }

        Write-Host "== undo (Ctrl+Z)"
        Focus-Window $hwnd
        Invoke-Undo
        Start-Sleep -Seconds 2
        $undo = Save-Shot $hwnd "$label-04-undo"
        [void](Test-Hung $app $hwnd "$label after undo")
        $back = Describe-Change $stroke $undo
        $left = Describe-Change $before $undo
        if ($app.HasExited) { Fail "undo ($label)" "the app exited with $($app.ExitCode)" }
        elseif ($back.Screen -gt 200 -or $back.Window -gt 200) { Add-Row "undo ($label)" 'PASS' "changed: $($back.Text); left vs before the stroke: $($left.Text)" }
        else { Input-Problem "undo ($label)" "Ctrl+Z changed almost nothing: $($back.Text)" }

        # 30 s of idling, with a stroke after each 10 s: the window must stay responsive (looked at
        # every 5 s) and must still draw afterwards, which an app stuck in a present would not.
        Write-Host "== idle 30 s with strokes"
        $last = $undo
        foreach ($round in 1..3) {
            foreach ($slice in 1..2) {
                Start-Sleep -Seconds 5
                $app.Refresh()
                if ($app.HasExited) { break }
                [void](Test-Hung $app $hwnd "$label idle round $round")
            }
            if ($app.HasExited) { break }
            if ($round -lt 3) {
                Focus-Window $hwnd
                $y = 0.30 + 0.15 * $round
                $beforeStroke = Save-Shot $hwnd "$label-05-idle-$round"
                Invoke-Stroke $hwnd $y
                Start-Sleep -Seconds 1
                $afterStroke = Save-Shot $hwnd "$label-06-stroke-after-idle-$round"
                $c = Describe-Change $beforeStroke $afterStroke
                if ($c.Screen -gt 200 -or $c.Window -gt 200) { Add-Row "stroke after idle $round ($label)" 'PASS' "changed: $($c.Text)" }
                else { Input-Problem "stroke after idle $round ($label)" "the window did not draw the stroke: $($c.Text)" }
            }
        }
        if ($app.HasExited) { Fail "idle ($label)" "the app exited with $($app.ExitCode)" }
        else { Add-Row "idle 30 s ($label)" 'PASS' "still running, $([math]::Round($app.WorkingSet64 / 1MB)) MB working set" }
        [void](Save-Shot $hwnd "$label-07-end")
        $stall = Find-LogLine 'STALL|surface: frame skipped|GPU compositor stopped'
        if ($stall) { Warn "driver notes ($label)" $stall }
    }
    finally {
        if (-not $app.HasExited) {
            Write-Host "== close"
            [void]$app.CloseMainWindow()
            # The app ends the close by itself after about 2 s (it has no unsaved-changes prompt:
            # work is saved on the way out), so 15 s is generous.
            $closeTimer = [System.Diagnostics.Stopwatch]::StartNew()
            if (-not $app.WaitForExit(15000)) {
                # Something kept it open: record what is on screen, then stop it.
                [void](Save-Screen "$label-08-close-blocked")
                $app.Kill()
                $app.WaitForExit()
                Fail "close ($label)" "the window did not close within 15 s of WM_CLOSE; killed (see $label-08-close-blocked-screen.png and the exit: lines in $label-startup.log)"
            }
            else {
                $app.WaitForExit()
                if ($app.ExitCode -eq 0) { Add-Row "close ($label)" 'PASS' "exit 0 after $([math]::Round($closeTimer.Elapsed.TotalSeconds, 1)) s" } else { Fail "close ($label)" "exit $($app.ExitCode). $(Tail (Join-Path $OutDir "window-$label.err.log"))" }
            }
        }
        Save-StartupLog $label
    }
}

# `--renderer gl` asks for OpenGL (glow). GitHub's Windows VM only has OpenGL 1.1, which eskiz's OpenGL
# renderer cannot use, so Eskiz falls back to the software rung. That is not a test of OpenGL: the
# step says so (SKIP with the reason) instead of passing the software path under the OpenGL name.
function Invoke-GlRun {
    Write-Host "== window with --renderer gl"
    if (-not (Test-Path $sample)) { Fail 'window (--renderer gl)' "no $sample"; return }
    Reset-Logs
    $gl = Start-Eskiz 'window-gl' @('--renderer', 'gl', "`"$sample`"")
    try {
        $deadline = (Get-Date).AddSeconds($WindowTimeout)
        while ((Get-Date) -lt $deadline -and -not $gl.HasExited) {
            $gl.Refresh()
            if ($gl.MainWindowHandle -ne [IntPtr]::Zero) { break }
            Start-Sleep -Milliseconds 250
        }
        $h = $gl.MainWindowHandle
        if ($gl.HasExited) { Fail 'window (--renderer gl)' "exited with $($gl.ExitCode) before its window appeared. $(Tail (Join-Path $OutDir 'window-gl.err.log'))"; return }
        if ($h -eq [IntPtr]::Zero) { Fail 'window (--renderer gl)' "no window after $WindowTimeout s"; return }
        Start-Sleep -Seconds 6
        [void](Save-Shot $h 'gl-01-opened')
        $hung = Test-Hung $gl $h '--renderer gl'
        $chosen = Find-LogLine 'renderer: '
        if ($chosen -match 'glow|OpenGL') {
            if (-not $hung) { Add-Row 'window (--renderer gl)' 'PASS' "OpenGL, responsive. $chosen" }
        }
        else {
            $why = Find-LogLine 'glow|OpenGL|GL_'
            Warn 'window (--renderer gl)' "SKIP: OpenGL could not be tested on this machine (no usable OpenGL 2.0 here); Eskiz fell back to: $chosen. $why"
        }
    }
    finally {
        if (-not $gl.HasExited) {
            [void]$gl.CloseMainWindow()
            if (-not $gl.WaitForExit(15000)) { $gl.Kill(); $gl.WaitForExit() }
        }
        Save-StartupLog 'gl'
    }
}

try {
    Remove-Item -Force -Path $sample -ErrorAction SilentlyContinue
    $os = Get-CimInstance Win32_OperatingSystem
    $screen = [System.Windows.Forms.SystemInformation]::VirtualScreen
    Add-Row 'machine' 'info' "$($os.Caption) $($os.Version), screen $($screen.Width)x$($screen.Height), $env:NUMBER_OF_PROCESSORS CPUs"

    [void](Invoke-Eskiz 'version' @('--version') 60)
    [void](Invoke-Eskiz 'selftest' @('--selftest') 900)
    [void](Invoke-Eskiz 'diagnose' @('--diagnose') 300)
    if (Test-Path (Join-Path $OutDir 'eskiz-diagnose.md')) {
        Move-Item -Force (Join-Path $OutDir 'eskiz-diagnose.md') (Join-Path $OutDir 'diagnose.md')
    }
    [void](Invoke-Eskiz 'diagnose-window' @('--diagnose', '--window') 300)
    if (Test-Path (Join-Path $OutDir 'eskiz-diagnose.md')) {
        Move-Item -Force (Join-Path $OutDir 'eskiz-diagnose.md') (Join-Path $OutDir 'diagnose-window.md')
    }
}
catch {
    Fail 'smoke script' "$($_.Exception.Message) (line $($_.InvocationInfo.ScriptLineNumber))"
}

# Each run is independent: an error in one is recorded and the next still happens.
#   main      the default start, as a user gets it (automatic renderer, GPU compositor where the ladder allows)
#   dx12      `--renderer dx12`: DX12 with the GPU compositor, forced (the combination that froze)
#   software  `--renderer software`: the same WARP adapter with the CPU compositor
$runs = @(
    @{ Label = 'main'; Args = @(); Expect = 'wgpu' },
    @{ Label = 'dx12'; Args = @('--renderer', 'dx12'); Expect = 'wgpu-dx12' },
    @{ Label = 'software'; Args = @('--renderer', 'software'); Expect = 'wgpu-software' }
)
foreach ($r in $runs) {
    try { Invoke-WindowRun $r.Label $r.Args $r.Expect }
    catch { Fail "window run ($($r.Label))" "$($_.Exception.Message) (line $($_.InvocationInfo.ScriptLineNumber))" }
}
try { Invoke-GlRun } catch { Fail 'compat run' "$($_.Exception.Message) (line $($_.InvocationInfo.ScriptLineNumber))" }

# ------------------------------------------------------------------------------------- summary

$verdict = if ($failures.Count -eq 0) { 'PASS' } else { 'FAIL' }
$report = New-Object System.Collections.Generic.List[string]
$report.Add("## Windows smoke test: $verdict")
$report.Add('')
$report.Add("``$Exe``")
$report.Add('')
$report.Add('| step | result | detail |')
$report.Add('|---|---|---|')
foreach ($r in $rows) { $report.Add($r) }
$report.Add('')
$report.Add('Screenshots: `*-screen.png` (desktop) and `*-window.png` (PrintWindow) in the artifact.')
foreach ($f in 'diagnose-window.md', 'diagnose.md') {
    $path = Join-Path $OutDir $f
    if (Test-Path $path) {
        $report.Add('')
        $report.Add("<details><summary>$f</summary>")
        $report.Add('')
        foreach ($line in Get-Content -Path $path) { $report.Add($line) }
        $report.Add('')
        $report.Add('</details>')
        break
    }
}
$text = $report -join "`n"
Set-Content -Path (Join-Path $OutDir 'smoke-report.md') -Value $text -Encoding utf8
if ($env:GITHUB_STEP_SUMMARY) { Add-Content -Path $env:GITHUB_STEP_SUMMARY -Value $text -Encoding utf8 }
Write-Host ''
Write-Host $text

if ($failures.Count -gt 0) {
    Write-Host ''
    Write-Host "FAILED: $($failures -join '; ')"
    exit 1
}
