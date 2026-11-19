#Requires -Version 7

<#
.SYNOPSIS
  Refreshes the desktop UI screenshots (assets/) and the mobile screenshots (assets/mobile/),
  and with -Videos the tutorial videos (videos/). The desktop shots and the videos use the
  remote worker configured in this machine's pleochroma-cut settings when it is reachable, and
  stay on this computer when it is not.

.DESCRIPTION
  Before the desktop and video runs, the script reads [settings.remote.connection] of the
  settings file (this user's own unless -Settings names another) and tries a TCP connection to
  its address (3 s). When it answers:
    - `cargo ui-screens` runs with PLEOCHROMA_SCREENS_REMOTE set to the settings file. Every
      desktop shot that renders adds the remote's samples, except the shots that show the
      remote choices themselves (parameters_render, emerald_export_settings_4k,
      tilt_performance_export, remote_coordinator).
    - `cargo tutorial-videos` runs with PLEOCHROMA_VIDEOS_REMOTE set to the settings file.
      Every tutorial's live renders add the remote's samples, except the two Render Jobs
      lessons (output-render-jobs, output-render-jobs-queue), which show where a job renders.
  Live Compute is Local + Remote, and only the [settings.remote] table is used, under the
  neutral label "Render Server". When no remote is configured or it does not answer, both runs
  render on this computer and the script says so. The harnesses check the remote again
  themselves and stay local when it stops answering.

  The mobile run (`cargo ui-screens-mobile`) always renders on this computer.

  Each harness keeps going when a shot or video fails and leaves its old file in place. The
  script writes:
    <Logs>\<timestamp>\01-ui-screens.log          full output of the desktop run
    <Logs>\<timestamp>\02-ui-screens-mobile.log   full output of the mobile run
    <Logs>\<timestamp>\03-tutorial-videos.log     full output of the video run
    <Logs>\<timestamp>\summary.md                 exit code, duration and renderer of each run
    <Logs>\LATEST.txt                             path of the newest run folder
  The numbers count the runs that are not skipped. The harness reports are
  target/ui-screens/report.md, target/ui-screens-mobile/report.md and
  target/tutorial-videos/report.md.

.PARAMETER Library
  The facet_diagrams.sqlite to photograph (default: facet_diagrams.sqlite in the current
  folder). The desktop shots need it; the mobile library shots and the videos use it when it
  exists (the videos use a small fixture library otherwise).

.PARAMETER Settings
  Only needed to use another settings.toml for the remote. Default: this user's pleochroma-cut
  settings, %APPDATA%\pleochroma-cut\settings.toml on Windows, $XDG_CONFIG_HOME or ~/.config on
  Linux.

.PARAMETER Shots
  Name substrings that select desktop shots, e.g. main, parameters (default: every shot).

.PARAMETER MobileShots
  Name substrings that select mobile shots, e.g. phone_design, tablet (default: every shot).

.PARAMETER Tutorials
  Id substrings that select tutorials, e.g. welcome, render-jobs (default: every tutorial).
  Implies -Videos.

.PARAMETER Gpu
  Builds the desktop and video harnesses with --features gpu, so this computer's share renders
  on its GPU.

.PARAMETER VideoDebug
  Runs the videos in debug mode (PLEOCHROMA_VIDEOS_DEBUG): no video is kept, each tutorial
  leaves step screenshots and a step log in videos/debug/<id>/. Implies -Videos.

.PARAMETER Videos
  Adds the tutorial video run after the screenshot runs (off by default: a full run takes
  hours).

.PARAMETER SkipDesktop
  Skips the desktop screenshot run.

.PARAMETER SkipMobile
  Skips the mobile screenshot run.

.PARAMETER Repo
  The repository root (default: the folder above this script).

.PARAMETER Logs
  The folder for the run logs (default C:\temp\mcp2\owner_runs\screens on Windows,
  ~/owner_runs/screens elsewhere).

.EXAMPLE
  .\scripts\refresh-screens.ps1 -Gpu

.EXAMPLE
  .\scripts\refresh-screens.ps1 -Gpu -Videos

.EXAMPLE
  .\scripts\refresh-screens.ps1 -SkipDesktop -SkipMobile -Tutorials welcome -VideoDebug

.EXAMPLE
  .\scripts\refresh-screens.ps1 -Library D:\pleochroma\facet_diagrams.sqlite -Shots main, parameters
#>
param(
    [string]$Library = (Join-Path (Get-Location).Path 'facet_diagrams.sqlite'),
    [string]$Settings,
    [string[]]$Shots = @(),
    [string[]]$MobileShots = @(),
    [string[]]$Tutorials = @(),
    [switch]$Gpu,
    [switch]$VideoDebug,
    [switch]$SkipDesktop,
    [switch]$SkipMobile,
    [switch]$Videos,
    [string]$Repo = (Split-Path -Parent $PSScriptRoot),
    [string]$Logs
)

$ErrorActionPreference = 'Continue'
if ($Tutorials.Count -gt 0 -or $VideoDebug) { $Videos = [switch]$true }
if ($SkipDesktop -and $SkipMobile -and -not $Videos) {
    Write-Error 'Every run is skipped; nothing to do.'
    exit 2
}

# The address of [settings.remote.connection] in `path`, or $null when there is none.
function Get-RemoteAddress([string]$path) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { return $null }
    $inConnection = $false
    foreach ($line in Get-Content -LiteralPath $path) {
        if ($line -match '^\s*\[([^\]]+)\]\s*$') {
            $inConnection = $Matches[1].Trim() -eq 'settings.remote.connection'
            continue
        }
        if ($inConnection -and $line -match '^\s*address\s*=\s*"([^"]*)"') {
            $address = $Matches[1].Trim()
            if ($address) { return $address }
        }
    }
    return $null
}

# Whether `address` (host:port, IPv6 hosts in brackets) accepts a TCP connection within 3 s.
function Test-RemoteReachable([string]$address) {
    $cut = $address.LastIndexOf(':')
    if ($cut -lt 1) { return $false }
    $hostName = $address.Substring(0, $cut).Trim('[', ']')
    $port = 0
    if (-not [int]::TryParse($address.Substring($cut + 1), [ref]$port)) { return $false }
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $connect = $client.ConnectAsync($hostName, $port)
        return $connect.Wait(3000) -and $client.Connected
    } catch {
        return $false
    } finally {
        $client.Dispose()
    }
}

if (-not $Settings) {
    $configHome = if ($IsWindows) {
        $env:APPDATA
    } elseif ($env:XDG_CONFIG_HOME) {
        $env:XDG_CONFIG_HOME
    } else {
        Join-Path $HOME '.config'
    }
    $Settings = Join-Path (Join-Path $configHome 'pleochroma-cut') 'settings.toml'
} elseif (-not (Test-Path -LiteralPath $Settings -PathType Leaf)) {
    Write-Error "No settings file at ${Settings}."
    exit 2
}
if (-not $Logs) {
    $Logs = if ($IsWindows) { 'C:\temp\mcp2\owner_runs\screens' } else { Join-Path $HOME 'owner_runs/screens' }
}

$haveLibrary = Test-Path -LiteralPath $Library -PathType Leaf
if ($haveLibrary) {
    $Library = (Resolve-Path -LiteralPath $Library).Path
} elseif (-not $SkipDesktop) {
    Write-Error "No library at ${Library}: run from the folder holding facet_diagrams.sqlite or pass -Library."
    exit 2
} else {
    $videoNote = if ($Videos) { ' and the videos use the fixture library' } else { '' }
    Write-Host "No library at ${Library}: the mobile library shots show the empty state$videoNote."
}

$useRemote = $false
if (-not $SkipDesktop -or $Videos) {
    $runs = if ($SkipDesktop) {
        'the videos'
    } elseif ($Videos) {
        'the desktop shots and videos'
    } else {
        'the desktop shots'
    }
    $address = Get-RemoteAddress $Settings
    if (-not $address) {
        Write-Host "No remote is configured: $runs render on this computer."
    } elseif (Test-RemoteReachable $address) {
        $useRemote = $true
        Write-Host "The remote answers: $runs render on the remote and this computer."
    } else {
        Write-Host "The remote does not answer: $runs render on this computer."
    }
}

$stamp = Get-Date -Format 'yyyy-MM-dd_HHmmss'
$out = Join-Path $Logs $stamp
New-Item -ItemType Directory -Force -Path $out | Out-Null
Set-Content -Path (Join-Path $Logs 'LATEST.txt') -Value $out
$summary = Join-Path $out 'summary.md'
Set-Content -Path $summary -Value @(
    "# Screenshot and video refresh $stamp",
    '',
    "Repo: $Repo",
    '',
    '| Step | Name | Exit | Verdict | Minutes | Renders |',
    '|---|---|---|---|---|---|'
)

# The harness arguments: cargo's own flags first, then the filters after `--`.
$desktopArgs = @('ui-screens')
if ($Gpu) { $desktopArgs += @('--features', 'gpu') }
if ($Shots.Count -gt 0) { $desktopArgs += @('--') + $Shots }
$mobileArgs = @('ui-screens-mobile')
if ($MobileShots.Count -gt 0) { $mobileArgs += @('--') + $MobileShots }
$videoArgs = @('tutorial-videos')
if ($Gpu) { $videoArgs += @('--features', 'gpu') }
if ($Tutorials.Count -gt 0) { $videoArgs += @('--') + $Tutorials }

$remoteRenders = if ($useRemote) { 'remote + this computer' } else { 'this computer' }
$steps = @()
if (-not $SkipDesktop) {
    $steps += @{ Name = 'ui-screens'; Args = $desktopArgs; RemoteVar = 'PLEOCHROMA_SCREENS_REMOTE'; Renders = $remoteRenders }
}
if (-not $SkipMobile) {
    $steps += @{ Name = 'ui-screens-mobile'; Args = $mobileArgs; RemoteVar = $null; Renders = 'this computer' }
}
if ($Videos) {
    $steps += @{ Name = 'tutorial-videos'; Args = $videoArgs; RemoteVar = 'PLEOCHROMA_VIDEOS_REMOTE'; Renders = $remoteRenders }
}
$remoteVars = @('PLEOCHROMA_SCREENS_REMOTE', 'PLEOCHROMA_VIDEOS_REMOTE')
$failed = $false
$index = 0
foreach ($step in $steps) {
    $index += 1
    $tag = '[{0:D2}/{1:D2}]' -f $index, $steps.Count
    $log = Join-Path $out ('{0:D2}-{1}.log' -f $index, $step.Name)
    Write-Host "$tag cargo $($step.Args -join ' ')  (renders on $($step.Renders))"
    if ($haveLibrary) {
        $env:PLEOCHROMA_HARNESS_LIBRARY = $Library
    } else {
        Remove-Item Env:PLEOCHROMA_HARNESS_LIBRARY -ErrorAction SilentlyContinue
    }
    foreach ($name in $remoteVars) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
    if ($useRemote -and $step.RemoteVar) {
        Set-Item "Env:$($step.RemoteVar)" -Value $Settings
    }
    if ($step.Name -eq 'tutorial-videos' -and $VideoDebug) {
        $env:PLEOCHROMA_VIDEOS_DEBUG = '1'
    } else {
        Remove-Item Env:PLEOCHROMA_VIDEOS_DEBUG -ErrorAction SilentlyContinue
    }
    $sw = [Diagnostics.Stopwatch]::StartNew()
    Push-Location $Repo
    try {
        & cargo @($step.Args) *>&1 | Tee-Object -FilePath $log
        $code = $LASTEXITCODE
    } catch {
        $_ | Out-String | Add-Content -Path $log
        $code = -1
    } finally {
        Pop-Location
        foreach ($name in $remoteVars) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
        Remove-Item Env:PLEOCHROMA_VIDEOS_DEBUG -ErrorAction SilentlyContinue
    }
    $sw.Stop()
    $minutes = [math]::Round($sw.Elapsed.TotalMinutes, 1)
    $verdict = if ($code -eq 0) { 'ok' } else { 'FAILED' }
    if ($code -ne 0) { $failed = $true }
    Write-Host "$tag $verdict ($minutes min)"
    Add-Content -Path $summary -Value ('| {0} | {1} | {2} | {3} | {4} | {5} |' -f $tag, $step.Name, $code, $verdict, $minutes, $step.Renders)
}

Add-Content -Path $summary -Value @(
    '',
    'Harness reports: target/ui-screens/report.md, target/ui-screens-mobile/report.md, target/tutorial-videos/report.md'
)
Write-Host "Done. Summary: $summary"
if ($failed) { exit 1 }
