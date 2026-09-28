# End-to-end smoke test of the desktop app on Windows (the counterpart of
# run.sh, which needs sway and so only runs on Linux).
#
# Launches `npm run tauri dev` (debug build) with CHESSDB_SMOKE pointing at a
# driver script. The driver runs inside the webview, drives the UI through
# `window.__chessdb` and reports via `[smoke] …` log lines, ending with
# `[smoke] done` or `[smoke] FAILED: …`. Each `[smoke] shot:<name>` line is
# captured as <out-dir>\shots\<name>.png.
#
# Windows has no private headless display, so the window opens on the
# desktop. Screenshots use PrintWindow, so they show the app even when other
# windows cover it.
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts\smoke\run.ps1 [driver.js] [out-dir]
# (out-dir defaults to target\smoke\<timestamp>, which git ignores)
#
# Same placeholders and variables as run.sh: __ROOT__, __OUT__, __BIG__;
# SMOKE_TIMEOUT (default 900), SMOKE_PORT (default 1430), SMOKE_BIG.
param([string]$Driver, [string]$Out)
$ErrorActionPreference = 'Stop'

$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Driver) { $Driver = Join-Path $Root 'scripts\smoke\basic.js' }
if (-not $Out) { $Out = Join-Path $Root ('target\smoke\' + (Get-Date -Format 'yyyyMMdd-HHmmss')) }
$Timeout = if ($env:SMOKE_TIMEOUT) { [int]$env:SMOKE_TIMEOUT } else { 900 }
$Port = if ($env:SMOKE_PORT) { [int]$env:SMOKE_PORT } else { 1430 }
$Big = $env:SMOKE_BIG
if (-not $Big) { $Big = if (Test-Path (Join-Path $Root 'testdata\large\lichess_1m.pgn')) { '1' } else { '0' } }

$probe = New-Object System.Net.Sockets.TcpClient
try {
    $probe.Connect('127.0.0.1', $Port)
    Write-Error "smoke: port $Port is already in use; set SMOKE_PORT to a free port"
    exit 2
} catch [System.Net.Sockets.SocketException] {
} finally { $probe.Dispose() }

New-Item -ItemType Directory -Force -Path (Join-Path $Out 'shots') | Out-Null
$Out = (Resolve-Path $Out).Path
$utf8 = New-Object System.Text.UTF8Encoding($false)

# The harness window gets its own title, to tell it from a normal `tauri dev`.
$Title = "Chess Database smoke $Port"
$conf = Get-Content -Raw (Join-Path $Root 'app\src-tauri\tauri.conf.json') | ConvertFrom-Json
$windows = @($conf.app.windows)
foreach ($w in $windows) { $w.title = $Title }
$smokeConf = @{
    build = @{ devUrl = "http://localhost:$Port"; beforeDevCommand = "npm run dev -- --port $Port --strictPort" }
    app = @{ windows = $windows }
}
$confPath = Join-Path $Out 'tauri.smoke.json'
[IO.File]::WriteAllText($confPath, ($smokeConf | ConvertTo-Json -Depth 10), $utf8)

# Forward slashes: the drivers put these in JS strings and append "/name".
$js = [IO.File]::ReadAllText((Resolve-Path $Driver).Path)
$js = $js.Replace('__ROOT__', $Root.Replace('\', '/')).Replace('__OUT__', $Out.Replace('\', '/')).Replace('__BIG__', $Big)
$driverOut = Join-Path $Out 'driver.js'
[IO.File]::WriteAllText($driverOut, $js, $utf8)
$Log = Join-Path $Out 'dev.log'
[IO.File]::WriteAllText($Log, '')

Add-Type @'
using System;
using System.Drawing;
using System.Runtime.InteropServices;
public static class SmokeShot {
    [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr FindWindow(string cls, string title);
    [StructLayout(LayoutKind.Sequential)] struct RECT { public int L, T, R, B; }
    public static bool Take(string title, string path) {
        SetProcessDPIAware();
        IntPtr h = FindWindow(null, title);
        if (h == IntPtr.Zero) return false;
        RECT r; GetWindowRect(h, out r);
        using (var bmp = new Bitmap(r.R - r.L, r.B - r.T))
        using (var g = Graphics.FromImage(bmp)) {
            IntPtr dc = g.GetHdc();
            PrintWindow(h, dc, 2); // PW_RENDERFULLCONTENT: includes the WebView2 surface
            g.ReleaseHdc(dc);
            bmp.Save(path, System.Drawing.Imaging.ImageFormat.Png);
        }
        return true;
    }
}
'@ -ReferencedAssemblies System.Drawing

# Settings go to the output folder, so the user's own settings stay untouched.
$env:CHESSDB_SMOKE = $driverOut
$env:CHESSDB_CONFIG_DIR = Join-Path $Out 'config'
$app = Start-Process cmd.exe -PassThru -WindowStyle Hidden -WorkingDirectory (Join-Path $Root 'app') `
    -ArgumentList '/c', "npm run tauri dev -- --config `"$confPath`" > `"$Log`" 2>&1"

Write-Host "smoke: driver $Driver, output $Out (1M-game part: $Big)"
Write-Host 'smoke: the app window opens on your desktop'
$status = 1
$seen = 0
$start = Get-Date
$done = $false
try {
    while (-not $done -and ((Get-Date) - $start).TotalSeconds -lt $Timeout) {
        $fs = [IO.File]::Open($Log, 'Open', 'Read', 'ReadWrite,Delete')
        $text = (New-Object IO.StreamReader($fs, $utf8)).ReadToEnd()
        $fs.Dispose()
        $lines = @(($text -replace "\x1b\[[0-9;]*m", '') -split "`r?`n" | Where-Object { $_ -like '*`[smoke`]*' })
        for ($i = $seen; $i -lt $lines.Count; $i++) {
            $msg = $lines[$i].Substring($lines[$i].IndexOf('[smoke] ') + 8)
            Write-Host "  $msg"
            if ($msg -like 'shot:*') {
                $name = $msg.Substring(5) -replace '[^a-zA-Z0-9_-]', ''
                $png = Join-Path $Out "shots\$name.png"
                if ([SmokeShot]::Take($Title, $png)) { Write-Host "    screenshot: $png" }
            } elseif ($msg -eq 'done') { $status = 0; $done = $true; break }
            elseif ($msg -like 'FAILED*') { $done = $true; break }
        }
        $seen = $lines.Count
        if (-not $done -and $app.HasExited) { Write-Host 'smoke: the app exited before the driver finished'; break }
        if (-not $done) { Start-Sleep -Milliseconds 500 }
    }
    if (-not $done -and ((Get-Date) - $start).TotalSeconds -ge $Timeout) { Write-Host "smoke: timed out after ${Timeout}s" }
} finally {
    # npm, vite, cargo and the app all descend from this cmd.exe.
    if (-not $app.HasExited) { & taskkill.exe /T /F /PID $app.Id *> $null }
}

if ($status -eq 0) {
    Write-Host "smoke: passed (log: $Log)"
} else {
    Write-Host "smoke: FAILED (log: $Log)"
    Get-Content $Log | Select-String -Pattern 'error|panicked|ERROR' | Select-Object -Last 20 | ForEach-Object { $_.Line }
}
exit $status
