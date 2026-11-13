<#
.SYNOPSIS
    Runs the render-acceleration benchmark (guide section 7.4, steps 1, 3 and 5; step 2 with -Adaptive).

.DESCRIPTION
    For every scene of scenes.json it
      1. renders a reference REF (--sample-offset 0) and an independent second reference REF2
         (--sample-offset = the reference spp), both with --raw-out; REF also with --progress-raw
         for the convergence curve (stills only);
      2. compares REF with REF2 (the noise floor) and every progress snapshot with REF and REF2
         (changed area against samples per pixel);
      3. with -Adaptive: renders a fixed-spp BASELINE at the adaptive cap and an adaptive run for
         every target noise of both metrics, compares each with REF2 (threshold from REF), and
         applies the selection rule of section 7.4: a run passes when it is as good as the
         baseline (changed_area_pct <= baseline + 0.1, worst_tile_de <= 1.1 x baseline,
         glint_recall >= 0.99) on every scene.
      4. with -Beam: a fixed-spp run at the adaptive cap with --beam (beam tracing plus the
         adaptive residual), judged by the same rule against the baseline, with its seconds and the
         beam stats parsed from the "Beams: ..." note of the run's log (<scene>\beam_*.log);
      5. with -EngineMatrix (implies -Beam): the beam run repeated with --compute local and
         --local cpu, --local gpu and --local cpu+gpu (hybrid), each next to a fixed-spp baseline
         on the same engines, so the table shows the beam speed-up per engine.
    Steps 4 and 5 (the beam rows) need a CLI built with `--features beam`; beam tracing is
    retired. `bash scripts/bench_render_accel.sh --with-beam` builds one into target/beam.
    Everything is written to results.json in the output folder and summarised in a table.

    Every render runs the CLI's own estimator, exit collection (the apps' estimator), so REF,
    REF2 and the baselines are exit-collection renders. The beam rows render branching through
    their own flags.

    The script starts no build. Reference renders at 16k to 32k samples take a long time:
    run it on a quiet machine, and use -Scene to run a few scenes at a time.

.PARAMETER Cli
    The pleochroma-cli executable. Default: pleochroma-cli on the PATH.

.PARAMETER OutDir
    The output folder. Default: out next to this script.

.PARAMETER Scene
    Scene ids to run (see scenes.json). Default: all of them.

.PARAMETER RenderArgs
    Extra arguments for every render / tilt-video call, for example --local gpu.

.PARAMETER Adaptive
    Also run step 2, the adaptive runs. Needs a CLI built with `--features adaptive-sampling`
    (it has --adaptive, --target-noise X, --noise-metric relative|absolute on render / tilt-video);
    the script stops with a message when the CLI has none.

.PARAMETER Beam
    Also run the beam step. Needs a CLI built with `--features beam` (it has --beam on
    render / tilt-video); the script stops with a message when the CLI has none.

.PARAMETER BeamArgs
    Extra arguments for the beam runs only, for example --beam-residual-min-spp, 64.

.PARAMETER EngineMatrix
    Also run the engine matrix (implies -Beam, so the beam CLI). Do not put --local or --compute in -RenderArgs
    with it: the matrix sets them itself.

.PARAMETER DryRun
    Print the render commands of every scene and run nothing (no CLI needed).
#>
[CmdletBinding()]
param(
    [string]$Cli = 'pleochroma-cli',
    [string]$OutDir = (Join-Path $PSScriptRoot 'out'),
    [string[]]$Scene = @(),
    [string[]]$RenderArgs = @(),
    [switch]$Adaptive,
    [switch]$Beam,
    [string[]]$BeamArgs = @(),
    [switch]$EngineMatrix,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$Invariant = [System.Globalization.CultureInfo]::InvariantCulture
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)

# The selection rule of section 7.4, step 4: a run against the scene's fixed-spp baseline at the
# same cap, both compared with REF2 (which shares no samples with either).
$ChangedAreaMargin = 0.1
$WorstTileFactor = 1.1
$MinGlintRecall = 0.99

# The adaptive targets of section 7.4, step 2: relative error as a fraction, absolute as Delta E.
$AdaptiveTargets = @(
    @{ metric = 'relative'; values = @(0.02, 0.015, 0.01, 0.007, 0.005) },
    @{ metric = 'absolute'; values = @(3.0, 2.0, 1.5, 1.0, 0.7) }
)

if ($EngineMatrix) { $Beam = [switch]$true }

# The engine flags of the matrix rows: the label and the flags.
$MatrixRows = @(
    @{ label = 'cpu'; flags = @('--compute', 'local', '--local', 'cpu') },
    @{ label = 'gpu'; flags = @('--compute', 'local', '--local', 'gpu') },
    @{ label = 'hybrid'; flags = @('--compute', 'local', '--local', 'cpu+gpu') }
)

if (-not $DryRun -and -not (Get-Command $Cli -ErrorAction SilentlyContinue)) {
    throw "Cannot find '$Cli'. Give the path of pleochroma-cli with -Cli."
}

# Beam tracing is retired: only a CLI built with `--features beam` has --beam.
if (($Beam -or $Adaptive) -and -not $DryRun) {
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'   # the CLI's stderr lines must not become terminating errors
    $CliHelp = & $Cli render --help 2>&1 | Out-String
    $ErrorActionPreference = $previousPreference
    if ($Beam -and -not ([string]$CliHelp).Contains('--beam')) {
        Write-Error -ErrorAction Continue 'this CLI has no beam tracer: build pleochroma-cli with --features beam (beam tracing is retired)'
        exit 1
    }
    # Adaptive sampling is an optional build feature (non-default).
    if ($Adaptive -and -not ([string]$CliHelp).Contains('--adaptive')) {
        Write-Error -ErrorAction Continue 'this CLI has no adaptive sampling: build pleochroma-cli with --features adaptive-sampling'
        exit 1
    }
}

function Invoke-Cli {
    # Runs the CLI, echoes the command line, and stops on a non-zero exit code. A dry run only echoes.
    param([Parameter(Mandatory)][string[]]$CliArgs)
    Write-Host ('> {0} {1}' -f $Cli, ($CliArgs -join ' '))
    if ($DryRun) { return @() }
    $output = & $Cli @CliArgs
    if ($LASTEXITCODE -ne 0) {
        throw ('{0} {1} exited with code {2}' -f $Cli, $CliArgs[0], $LASTEXITCODE)
    }
    return $output
}

function Invoke-CliLogged {
    # Invoke-Cli for renders: the CLI's progress and "note:" lines are on stderr, so they are
    # merged into the returned lines (for NAME.log and the beam stats) and still shown.
    param([Parameter(Mandatory)][string[]]$CliArgs)
    Write-Host ('> {0} {1}' -f $Cli, ($CliArgs -join ' '))
    if ($DryRun) { return @() }
    $output = & $Cli @CliArgs 2>&1 | ForEach-Object {
        if ($_ -is [System.Management.Automation.ErrorRecord]) { [Console]::Error.WriteLine("$_") }
        "$_"
    }
    if ($LASTEXITCODE -ne 0) {
        throw ('{0} {1} exited with code {2}' -f $Cli, $CliArgs[0], $LASTEXITCODE)
    }
    return $output
}

function Compare-Raw {
    # image-compare --json: returns the parsed report.
    param(
        [Parameter(Mandatory)][string]$Ref,
        [Parameter(Mandatory)][string]$Run,
        [Parameter(Mandatory)][string]$Job,
        [string]$Ref2,
        [string]$ErrorPng
    )
    $compareArgs = @('image-compare', $Ref, $Run, '--tonemap-from', $Job, '--json')
    if ($Ref2) { $compareArgs += @('--ref2', $Ref2) }
    if ($ErrorPng) { $compareArgs += @('--error-png', $ErrorPng) }
    $json = Invoke-Cli -CliArgs $compareArgs
    return ($json -join "`n") | ConvertFrom-Json
}

function Select-Figures {
    # The numbers kept in results.json for one comparison.
    param([Parameter(Mandatory)]$Report)
    return [ordered]@{
        changed_area_pct = $Report.changed_area_pct
        worst_tile_de    = $Report.worst_tile_de
        glint_recall     = $Report.glint_recall
        mean_de          = $Report.mean_de
        p99_de           = $Report.p99_de
        max_spp          = $Report.max_spp
        samples_used     = $Report.samples_used
        samples_saving   = $Report.samples_saving
    }
}

function Merge-Worst {
    # The worst of several comparisons (the frames of a tilt scene).
    param([Parameter(Mandatory)][object[]]$Figures)
    $worst = [ordered]@{
        changed_area_pct = ($Figures | ForEach-Object { $_.changed_area_pct } | Measure-Object -Maximum).Maximum
        worst_tile_de    = ($Figures | ForEach-Object { $_.worst_tile_de } | Measure-Object -Maximum).Maximum
        glint_recall     = ($Figures | ForEach-Object { $_.glint_recall } | Measure-Object -Minimum).Minimum
        mean_de          = ($Figures | ForEach-Object { $_.mean_de } | Measure-Object -Maximum).Maximum
        p99_de           = ($Figures | ForEach-Object { $_.p99_de } | Measure-Object -Maximum).Maximum
        max_spp          = ($Figures | ForEach-Object { $_.max_spp } | Measure-Object -Maximum).Maximum
        samples_used     = ($Figures | ForEach-Object { $_.samples_used } | Measure-Object -Sum).Sum
        samples_saving   = ($Figures | ForEach-Object { $_.samples_saving } | Measure-Object -Minimum).Minimum
    }
    return $worst
}

function Get-RawFiles {
    # The .plraw files of a folder, in name order.
    param([Parameter(Mandatory)][string]$Folder, [string]$Filter = '*.plraw')
    return @(Get-ChildItem -LiteralPath $Folder -Filter $Filter -File | Sort-Object Name)
}

function Test-Passes {
    # The selection rule of section 7.4 for one run's figures against the scene's baseline.
    param([Parameter(Mandatory)]$Figures, [Parameter(Mandatory)]$Baseline)
    return ($Figures.changed_area_pct -le ($Baseline.changed_area_pct + $ChangedAreaMargin)) -and
        ($Figures.worst_tile_de -le ($Baseline.worst_tile_de * $WorstTileFactor)) -and
        # A null recall (the reference has no glints) has nothing to lose and passes.
        (($null -eq $Figures.glint_recall) -or ($Figures.glint_recall -ge $MinGlintRecall))
}

function New-AdaptiveJob {
    # A copy of the job with the adaptive sample cap in place of the reference count (no BOM).
    param([Parameter(Mandatory)]$SceneEntry, [Parameter(Mandatory)][string]$JobPath, [Parameter(Mandatory)][string]$Folder)
    if ([int]$SceneEntry.adaptive_max_spp -eq [int]$SceneEntry.reference_spp) { return $JobPath }
    $text = [System.IO.File]::ReadAllText($JobPath)
    $text = [regex]::Replace($text, '"samples_per_pixel":\s*\d+', ('"samples_per_pixel": {0}' -f [int]$SceneEntry.adaptive_max_spp))
    $copy = Join-Path $Folder 'adaptive.job.json'
    [System.IO.File]::WriteAllText($copy, $text, $Utf8NoBom)
    return $copy
}

function Invoke-Render {
    # One render of a scene: a still to FILE.plraw, or the frames of a tilt video to a folder.
    param(
        [Parameter(Mandatory)]$SceneEntry,
        [Parameter(Mandatory)][string]$JobPath,
        [Parameter(Mandatory)][string]$Folder,
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][long]$SampleOffset,
        [string[]]$Extra = @(),
        [string]$ProgressFolder
    )
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $named = @($SceneEntry.render_args) + $RenderArgs + $Extra
    $common = $named + @('--sample-offset', "$SampleOffset")
    # The CLI's output (progress and "note:" lines) is kept as NAME.log: the beam stats come from it.
    $log = Join-Path $Folder ($Name + '.log')
    if ($SceneEntry.kind -eq 'tilt_video') {
        $raw = Join-Path $Folder ($Name + '_raw')
        if (-not $DryRun) { New-Item -ItemType Directory -Force -Path $raw | Out-Null }
        $frames = Join-Path $Folder ($Name + '_frames')
        $output = Invoke-CliLogged -CliArgs (@('tilt-video', $JobPath, '--out-dir', $frames, '--restart', '--raw-out', $raw) + $common)
        if (-not $DryRun) { [System.IO.File]::WriteAllText($log, (($output | ForEach-Object { "$_" }) -join "`n") + "`n", $Utf8NoBom) }
        $script:LastSeconds = [math]::Round($watch.Elapsed.TotalSeconds, 1)
        return $raw
    }
    $raw = Join-Path $Folder ($Name + '.plraw')
    $png = Join-Path $Folder ($Name + '.png')
    $stillArgs = @('render', $JobPath, '--out', $png, '--raw-out', $raw)
    if ($ProgressFolder) {
        if (-not $DryRun) { New-Item -ItemType Directory -Force -Path $ProgressFolder | Out-Null }
        $stillArgs += @('--progress-raw', $ProgressFolder)
    }
    $output = Invoke-CliLogged -CliArgs ($stillArgs + $common)
    if (-not $DryRun) { [System.IO.File]::WriteAllText($log, (($output | ForEach-Object { "$_" }) -join "`n") + "`n", $Utf8NoBom) }
    $script:LastSeconds = [math]::Round($watch.Elapsed.TotalSeconds, 1)
    return $raw
}

function Get-BeamStats {
    # The beam stats of a run, read from its log: the "Beams: N leaves, X % residual" note (null
    # when the run printed none) and every note line.
    param([Parameter(Mandatory)][string]$Folder, [Parameter(Mandatory)][string]$Name)
    $lines = @(Get-Content -LiteralPath (Join-Path $Folder ($Name + '.log')))
    $beamLine = @($lines | Where-Object { $_ -match 'Beams:' }) | Select-Object -Last 1
    $leaves = $null
    $residual = $null
    # An old abbreviated "142k" form is read as 142000.
    if ($beamLine -match 'Beams:\s+(\d+)(k?)\s+leaves,\s+([0-9.]+)\s+%\s+residual') {
        $leaves = [long]$Matches[1]
        if ($Matches[2] -eq 'k') { $leaves = $leaves * 1000 }
        $residual = [double]::Parse($Matches[3], $Invariant)
    }
    return [ordered]@{
        beams_line   = $beamLine
        leaves       = $leaves
        residual_pct = $residual
        notes        = @($lines | Where-Object { $_ -match '^note:' })
    }
}

function Invoke-BeamRow {
    # One beam render at the adaptive cap, judged against the baseline; returns one row of "beam".
    param(
        [Parameter(Mandatory)]$SceneEntry,
        [Parameter(Mandatory)][string]$RunJob,
        [Parameter(Mandatory)][string]$Job,
        [Parameter(Mandatory)][string]$Folder,
        [Parameter(Mandatory)][string]$Ref,
        [Parameter(Mandatory)][string]$Ref2,
        [Parameter(Mandatory)]$Baseline,
        [Parameter(Mandatory)][string]$Mode,
        [Parameter(Mandatory)][string]$Engine,
        [string[]]$EngineFlags = @(),
        $EngineBaseline = $null
    )
    $name = 'beam_' + $Mode
    $run = Invoke-Render -SceneEntry $SceneEntry -JobPath $RunJob -Folder $Folder -Name $name -SampleOffset 0 -Extra ($EngineFlags + @('--beam') + $BeamArgs)
    $figures = Compare-Scene -SceneEntry $SceneEntry -Ref $Ref2 -Run $run -Job $Job -Ref2 $Ref -ErrorPngPrefix (Join-Path $Folder ($name + '_error'))
    $figures['seconds'] = $script:LastSeconds
    return [ordered]@{
        mode            = $Mode
        engine          = $Engine
        passes          = (Test-Passes -Figures $figures -Baseline $Baseline)
        figures         = $figures
        engine_baseline = $EngineBaseline
        stats           = (Get-BeamStats -Folder $Folder -Name $name)
    }
}

function Compare-Scene {
    # Compares REF with RUN for a scene: one report for a still, the worst of all frames for a tilt scene.
    param(
        [Parameter(Mandatory)]$SceneEntry,
        [Parameter(Mandatory)][string]$Ref,
        [Parameter(Mandatory)][string]$Run,
        [Parameter(Mandatory)][string]$Job,
        [string]$Ref2,
        [string]$ErrorPngPrefix
    )
    if ($SceneEntry.kind -ne 'tilt_video') {
        $png = if ($ErrorPngPrefix) { $ErrorPngPrefix + '.png' } else { $null }
        $report = Compare-Raw -Ref $Ref -Run $Run -Job $Job -Ref2 $Ref2 -ErrorPng $png
        return Select-Figures -Report $report
    }
    $perFrame = @()
    foreach ($refFrame in (Get-RawFiles -Folder $Ref)) {
        $runFrame = Join-Path $Run $refFrame.Name
        $ref2Frame = if ($Ref2) { Join-Path $Ref2 $refFrame.Name } else { $null }
        $png = if ($ErrorPngPrefix) { '{0}_{1}.png' -f $ErrorPngPrefix, [System.IO.Path]::GetFileNameWithoutExtension($refFrame.Name) } else { $null }
        $report = Compare-Raw -Ref $refFrame.FullName -Run $runFrame -Job $Job -Ref2 $ref2Frame -ErrorPng $png
        $perFrame += , (Select-Figures -Report $report)
    }
    if ($perFrame.Count -eq 0) { throw "No frames found in $Ref" }
    return Merge-Worst -Figures $perFrame
}

# ---- scenes ---------------------------------------------------------------------------------

$scenes = (Get-Content -LiteralPath (Join-Path $PSScriptRoot 'scenes.json') -Raw | ConvertFrom-Json).scenes
if ($Scene.Count -gt 0) {
    $scenes = @($scenes | Where-Object { $Scene -contains $_.id })
    if ($scenes.Count -eq 0) { throw "No scene matches: $($Scene -join ', ')" }
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$results = [ordered]@{
    generated = (Get-Date).ToString('o', $Invariant)
    cli       = $Cli
    rule      = [ordered]@{
        against                                = 'REF2; baseline = fixed spp at adaptive_max_spp'
        changed_area_pct_at_most_baseline_plus = $ChangedAreaMargin
        worst_tile_de_at_most_baseline_times   = $WorstTileFactor
        glint_recall_at_least                  = $MinGlintRecall
    }
    scenes    = @()
}

foreach ($entry in $scenes) {
    Write-Host ''
    Write-Host ('=== {0}: {1}' -f $entry.id, $entry.title)
    $folder = Join-Path $OutDir $entry.id
    $job = Join-Path $PSScriptRoot $entry.job
    $referenceSpp = [long]$entry.reference_spp
    $isTilt = $entry.kind -eq 'tilt_video'

    if ($DryRun) {
        # Only the render commands, in the order a real run issues them.
        $dryProgress = if ($isTilt) { $null } else { Join-Path $folder 'progress' }
        $dryJob = if ([int]$entry.adaptive_max_spp -eq [int]$entry.reference_spp) { $job } else { Join-Path $folder 'adaptive.job.json' }
        Invoke-Render -SceneEntry $entry -JobPath $job -Folder $folder -Name 'ref' -SampleOffset 0 -ProgressFolder $dryProgress | Out-Null
        Invoke-Render -SceneEntry $entry -JobPath $job -Folder $folder -Name 'ref2' -SampleOffset $referenceSpp | Out-Null
        Invoke-Render -SceneEntry $entry -JobPath $dryJob -Folder $folder -Name 'baseline' -SampleOffset 0 | Out-Null
        if ($Beam) {
            Invoke-Render -SceneEntry $entry -JobPath $dryJob -Folder $folder -Name 'beam_fixed' -SampleOffset 0 -Extra (@('--beam') + $BeamArgs) | Out-Null
        }
        if ($EngineMatrix) {
            foreach ($row in $MatrixRows) {
                Invoke-Render -SceneEntry $entry -JobPath $dryJob -Folder $folder -Name ('baseline_' + $row.label) -SampleOffset 0 -Extra $row.flags | Out-Null
                Invoke-Render -SceneEntry $entry -JobPath $dryJob -Folder $folder -Name ('beam_' + $row.label) -SampleOffset 0 -Extra ($row.flags + @('--beam') + $BeamArgs) | Out-Null
            }
        }
        continue
    }

    if (Test-Path -LiteralPath $folder) { Remove-Item -LiteralPath $folder -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $folder | Out-Null

    # Step 1: two independent references.
    $progress = if ($isTilt) { $null } else { Join-Path $folder 'progress' }
    $ref = Invoke-Render -SceneEntry $entry -JobPath $job -Folder $folder -Name 'ref' -SampleOffset 0 -ProgressFolder $progress
    $ref2 = Invoke-Render -SceneEntry $entry -JobPath $job -Folder $folder -Name 'ref2' -SampleOffset $referenceSpp

    # Step 3: the noise floor, REF against REF2.
    $noise = Compare-Scene -SceneEntry $entry -Ref $ref -Run $ref2 -Job $job -ErrorPngPrefix (Join-Path $folder 'noise_floor')

    # Step 5: the convergence curve from the progress snapshots (stills only).
    $curve = @()
    if (-not $isTilt) {
        foreach ($snapshot in (Get-RawFiles -Folder $progress)) {
            $vsRef = Compare-Raw -Ref $ref -Run $snapshot.FullName -Job $job -Ref2 $ref2
            $vsRef2 = Compare-Raw -Ref $ref2 -Run $snapshot.FullName -Job $job -Ref2 $ref
            $curve += , [ordered]@{
                file    = $snapshot.Name
                spp     = $vsRef.max_spp
                vs_ref  = Select-Figures -Report $vsRef
                vs_ref2 = Select-Figures -Report $vsRef2
            }
        }
    }

    $record = [ordered]@{
        id            = $entry.id
        kind          = $entry.kind
        reference_spp = $referenceSpp
        noise_floor   = $noise
        convergence   = $curve
        baseline      = $null
        adaptive      = @()
        beam          = @()
    }

    # The fixed-spp baseline at the cap (with -Adaptive or -Beam), then the beam runs
    # (-Beam, -EngineMatrix) and the adaptive renders for every target of both metrics.
    if ($Adaptive -or $Beam) {
        $adaptiveJob = New-AdaptiveJob -SceneEntry $entry -JobPath $job -Folder $folder
        $base = Invoke-Render -SceneEntry $entry -JobPath $adaptiveJob -Folder $folder -Name 'baseline' -SampleOffset 0
        $baseSeconds = $script:LastSeconds
        $baseline = Compare-Scene -SceneEntry $entry -Ref $ref2 -Run $base -Job $job -Ref2 $ref -ErrorPngPrefix (Join-Path $folder 'baseline_error')
        $baseline['seconds'] = $baseSeconds
        $record['baseline'] = $baseline
    }
    # Step 4 (only with -Beam): the same cap with beams on the engines of -RenderArgs.
    if ($Beam) {
        $rowCommon = @{ SceneEntry = $entry; RunJob = $adaptiveJob; Job = $job; Folder = $folder; Ref = $ref; Ref2 = $ref2; Baseline = $baseline }
        $record['beam'] += , (Invoke-BeamRow @rowCommon -Mode 'fixed' -Engine 'default')
    }
    # Step 5 (only with -EngineMatrix): fixed-spp and beam runs on CPU, GPU and hybrid.
    if ($EngineMatrix) {
        foreach ($row in $MatrixRows) {
            $engineName = 'baseline_' + $row.label
            $engineRun = Invoke-Render -SceneEntry $entry -JobPath $adaptiveJob -Folder $folder -Name $engineName -SampleOffset 0 -Extra $row.flags
            $engineSeconds = $script:LastSeconds
            $engineBaseline = Compare-Scene -SceneEntry $entry -Ref $ref2 -Run $engineRun -Job $job -Ref2 $ref -ErrorPngPrefix (Join-Path $folder ($engineName + '_error'))
            $engineBaseline['seconds'] = $engineSeconds
            $record['beam'] += , (Invoke-BeamRow @rowCommon -Mode $row.label -Engine $row.label -EngineFlags $row.flags -EngineBaseline $engineBaseline)
        }
    }
    if ($Adaptive) {
        foreach ($group in $AdaptiveTargets) {
            foreach ($target in $group.values) {
                $text = $target.ToString($Invariant)
                $name = 'adaptive_{0}_{1}' -f $group.metric, $text
                $extra = @('--adaptive', '--target-noise', $text, '--noise-metric', $group.metric)
                $run = Invoke-Render -SceneEntry $entry -JobPath $adaptiveJob -Folder $folder -Name $name -SampleOffset 0 -Extra $extra
                $figures = Compare-Scene -SceneEntry $entry -Ref $ref2 -Run $run -Job $job -Ref2 $ref -ErrorPngPrefix (Join-Path $folder ($name + '_error'))
                $record['adaptive'] += , ([ordered]@{ metric = $group.metric; target = $target; passes = (Test-Passes -Figures $figures -Baseline $baseline); figures = $figures })
            }
        }
    }
    $results['scenes'] += , $record
}

if ($DryRun) {
    Write-Host ''
    Write-Host 'Dry run: nothing was rendered or written.'
    return
}

# ---- selection rule (only with -Adaptive) --------------------------------------------------

$selection = [ordered]@{}
if ($Adaptive) {
    foreach ($group in $AdaptiveTargets) {
        $passing = @()
        foreach ($target in $group.values) {
            $rows = @($results.scenes | ForEach-Object { $_.adaptive | Where-Object { $_.metric -eq $group.metric -and $_.target -eq $target } })
            if ($rows.Count -eq $results.scenes.Count -and -not ($rows | Where-Object { -not $_.passes })) {
                $passing += $target
            }
        }
        if ($passing.Count -gt 0) {
            $loosest = ($passing | Measure-Object -Maximum).Maximum
            $savings = [ordered]@{}
            foreach ($sceneRecord in $results.scenes) {
                $row = $sceneRecord.adaptive | Where-Object { $_.metric -eq $group.metric -and $_.target -eq $loosest }
                $savings[$sceneRecord.id] = $row.figures.samples_saving
            }
            $selection[$group.metric] = [ordered]@{ loosest_target = $loosest; samples_saving = $savings }
        }
        else {
            $selection[$group.metric] = [ordered]@{ loosest_target = $null; note = 'no target passes on every scene' }
        }
    }
}
$results['selection'] = $selection

# ---- results.json and summary --------------------------------------------------------------

$resultsPath = Join-Path $OutDir 'results.json'
$json = ConvertTo-Json -InputObject $results -Depth 12
[System.IO.File]::WriteAllText($resultsPath, $json + "`n", $Utf8NoBom)

Write-Host ''
Write-Host 'Noise floor (REF against REF2; the worst frame for a tilt scene)'
$results.scenes | ForEach-Object {
    [pscustomobject]@{
        scene            = $_.id
        reference_spp    = $_.reference_spp
        mean_de          = $_.noise_floor.mean_de
        p99_de           = $_.noise_floor.p99_de
        changed_area_pct = $_.noise_floor.changed_area_pct
        worst_tile_de    = $_.noise_floor.worst_tile_de
        glint_recall     = $_.noise_floor.glint_recall
    }
} | Format-Table -AutoSize

if ($Adaptive) {
    Write-Host 'Adaptive runs'
    foreach ($sceneRecord in $results.scenes) {
        $baselineRow = [ordered]@{
            metric  = 'baseline'
            target  = $sceneRecord.baseline.max_spp
            passes  = '-'
            figures = $sceneRecord.baseline
        }
        @($baselineRow) + @($sceneRecord.adaptive) | ForEach-Object {
            [pscustomobject]@{
                scene            = $sceneRecord.id
                metric           = $_.metric
                target           = $_.target
                passes           = $_.passes
                changed_area_pct = $_.figures.changed_area_pct
                worst_tile_de    = $_.figures.worst_tile_de
                glint_recall     = $_.figures.glint_recall
                samples_saving   = $_.figures.samples_saving
            }
        } | Format-Table -AutoSize
    }
    Write-Host 'Selection (loosest target that passes on every scene)'
    foreach ($metric in $selection.Keys) {
        Write-Host ('  {0}: {1}' -f $metric, ($selection[$metric] | ConvertTo-Json -Compress -Depth 6))
    }
}

if ($Beam) {
    Write-Host 'Beam runs (against the fixed-spp baseline at the same cap; the engines of -RenderArgs)'
    foreach ($sceneRecord in $results.scenes) {
        @($sceneRecord.beam | Where-Object { $_.engine -eq 'default' }) | ForEach-Object {
            [pscustomobject]@{
                scene            = $sceneRecord.id
                mode             = $_.mode
                passes           = $_.passes
                changed_area_pct = $_.figures.changed_area_pct
                worst_tile_de    = $_.figures.worst_tile_de
                glint_recall     = $_.figures.glint_recall
                seconds          = $_.figures.seconds
                baseline_seconds = $sceneRecord.baseline.seconds
                leaves           = $_.stats.leaves
                residual_pct     = $_.stats.residual_pct
            }
        } | Format-Table -AutoSize
    }
}

if ($EngineMatrix) {
    Write-Host 'Engine matrix (beam run against the fixed-spp run on the same engines; speedup = baseline seconds / beam seconds)'
    foreach ($sceneRecord in $results.scenes) {
        @($sceneRecord.beam | Where-Object { $_.engine -ne 'default' }) | ForEach-Object {
            [pscustomobject]@{
                scene            = $sceneRecord.id
                engine           = $_.engine
                passes           = $_.passes
                changed_area_pct = $_.figures.changed_area_pct
                worst_tile_de    = $_.figures.worst_tile_de
                glint_recall     = $_.figures.glint_recall
                beam_seconds     = $_.figures.seconds
                baseline_seconds = $_.engine_baseline.seconds
                speedup          = if ($_.figures.seconds -gt 0) { [math]::Round($_.engine_baseline.seconds / $_.figures.seconds, 2) } else { $null }
                leaves           = $_.stats.leaves
                residual_pct     = $_.stats.residual_pct
            }
        } | Format-Table -AutoSize
    }
}

Write-Host ''
Write-Host "Wrote $resultsPath"
