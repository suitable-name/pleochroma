#Requires -Version 7
<#
.SYNOPSIS
  Runs the Windows-reference pin commands of the 2026-10-26 campaign and logs everything.

.DESCRIPTION
  Runs each command from the repo root, keeps going when one fails, and writes:
    C:\temp\mcp2\owner_runs\windows\<timestamp>\<nn>-<name>.log   full output of each command
    C:\temp\mcp2\owner_runs\windows\<timestamp>\summary.md        exit code and duration per command
    C:\temp\mcp2\owner_runs\windows\LATEST.txt                    path of the newest run folder
  Nothing is re-pinned by this script: the printed values are read afterwards.

.PARAMETER Repo
  The repository root (default C:\Users\ngussek\Dev\Rust\diagram_loader).

.PARAMETER SkipGpu
  Skip every step marked Gpu (none at the moment; kept for steps that need an adapter).
#>
param(
    [string]$Repo = 'C:\Users\ngussek\Dev\Rust\diagram_loader',
    [switch]$SkipGpu
)

$ErrorActionPreference = 'Continue'
$stamp = Get-Date -Format 'yyyy-MM-dd_HHmmss'
$root = 'C:\temp\mcp2\owner_runs\windows'
$out = Join-Path $root $stamp
New-Item -ItemType Directory -Force -Path $out | Out-Null
Set-Content -Path (Join-Path $root 'LATEST.txt') -Value $out
$summary = Join-Path $out 'summary.md'

Set-Content -Path $summary -Value @(
    "# Windows pin run $stamp",
    '',
    "Repo: $Repo",
    '',
    '| Step | Name | Exit | Verdict | Minutes | Command |',
    '|---|---|---|---|---|---|'
)

# Every step, in order; the count gives each line its [xx/yy]. Steps that passed on the
# owner's run 2026-10-27_010333 and need no further pin are gone (optimize views, model widths,
# face-up path, raytracer goldens, event-wavefront GPU tests, both golden captures); only the
# two pins re-recorded from that run remain, to confirm them. Add a step back when a change
# needs it (for example a WGSL edit: the ew_ tests and the golden captures).
$steps = @(
    @{ Name = 'form-parsing-pin'; Gpu = $false
       Display = 'cargo test -p pleochroma-cut --lib form_parsing_is_pinned -- --nocapture'
       Command = { cargo test -p pleochroma-cut --lib form_parsing_is_pinned -- --nocapture } }
    # Color variants campaign (2026-10-27, lane B1): Zircon got its measured per-axis tensor, so
    # `cpu_accumulate_zircon_frosted_daylight_pin` moves (both hashes); the Diamond pin stays.
    @{ Name = 'cpu-accumulate-pins'; Gpu = $false
       Display = 'cargo test -p pleochroma --features gpu --lib renderer::gpu::pin_tests -- --nocapture'
       Command = { cargo test -p pleochroma --features gpu --lib renderer::gpu::pin_tests -- --nocapture } }
    # Zoned-opal GPU-vs-CPU gap (reviews/FABLE_zoned_opal.md section 7) on this machine's AMD
    # driver, to set against the Linux run: the zoned GPU tests with their printed ratios, and the
    # ignored six-row diagnostic.
    @{ Name = 'zoned-gpu-tests'; Gpu = $true
       Display = 'cargo test -p pleochroma --all-features --lib gpu_backend::zoned_tests -- --nocapture'
       Command = { cargo test -p pleochroma --all-features --lib gpu_backend::zoned_tests -- --nocapture } }
    @{ Name = 'zoned-opal-diagnosis'; Gpu = $true
       Display = 'cargo test -p pleochroma --all-features --lib zoned_opal_gap_diagnosis -- --ignored --nocapture'
       Command = { cargo test -p pleochroma --all-features --lib zoned_opal_gap_diagnosis -- --ignored --nocapture } }
    # The cut pins re-recorded from the lead's Windows run 2026-10-27 (cutting sheet fixture 0,
    # retarget dump), to confirm them.
    @{ Name = 'cut-identity-pins'; Gpu = $false
       Display = 'cargo test -p pleochroma-cut --lib identity_pins -- --nocapture'
       Command = { cargo test -p pleochroma-cut --lib identity_pins -- --nocapture } }
    # Color variants campaign (2026-10-27): the raytracer tests that use the per-axis built-ins
    # (Topaz in the biaxial refraction check, Tourmaline orientation, Zircon defaults); the
    # Diamond/Sapphire goldens must stay bit-identical. Then the campaign's own tests (variant
    # data with its pinned face-up colors, keyed color list, per-axis editor, patterns, vault).
    @{ Name = 'raytracer-tests'; Gpu = $false
       Display = 'cargo test -p pleochroma --test raytracer_tests -- --nocapture'
       Command = { cargo test -p pleochroma --test raytracer_tests -- --nocapture } }
    @{ Name = 'color-variants-tests'; Gpu = $false
       Display = 'cargo test -p pleochroma -p pleochroma-editor -p pleochroma-cut-core -p pleochroma-cut -p pleochroma-web-core -p pleochroma-mobile-core -p pleochroma-vault --lib -- <color filters>'
       Command = { cargo test -p pleochroma -p pleochroma-editor -p pleochroma-cut-core -p pleochroma-cut -p pleochroma-web-core -p pleochroma-mobile-core -p pleochroma-vault --lib -- variants built_in_hues color_entry lch_color variant_choice color_variant pattern render_body_color body_color_combo render_color materials_symmetry scene_identity design_color design_props pattern_wire } }
    # No WGSL change in the color variants campaign (KERNEL_SET_HASH stays); the GPU tests and the
    # golden captures run again because the per-axis built-ins (Zircon, Topaz, Tourmaline,
    # Amethyst, Citrine, Morganite, Chrysoberyl (Yellow), Andalusite) change scenes that use them.
    # New capture folders, so the 2026-10-27 morning captures stay for comparison.
    @{ Name = 'gpu-ew-tests'; Gpu = $true
       Display = 'cargo test -p pleochroma --features gpu --release ew_ -- --nocapture'
       Command = { cargo test -p pleochroma --features gpu --release ew_ -- --nocapture } }
    @{ Name = 'gpu-golden-capture'; Gpu = $true
       Display = 'cargo run --profile probe -p pleochroma --features gpu --example gpu_golden_capture -- capture --out target/gpu_golden/after_color_variants_2026-10-27'
       Command = { cargo run --profile probe -p pleochroma --features gpu --example gpu_golden_capture -- capture --out target/gpu_golden/after_color_variants_2026-10-27 } }
    @{ Name = 'gpu-golden-capture-event'; Gpu = $true
       Display = 'cargo run --profile probe -p pleochroma --features gpu --example gpu_golden_capture -- capture --out target/gpu_golden/after_color_variants_2026-10-27_event --pipeline event'
       Command = { cargo run --profile probe -p pleochroma --features gpu --example gpu_golden_capture -- capture --out target/gpu_golden/after_color_variants_2026-10-27_event --pipeline event } }
)
if ($SkipGpu) { $steps = @($steps | Where-Object { -not $_.Gpu }) }
$total = $steps.Count

$index = 0
foreach ($step in $steps) {
    $index += 1
    $tag = '[{0:D2}/{1:D2}]' -f $index, $total
    $log = Join-Path $out ('{0:D2}-{1}.log' -f $index, $step.Name)
    Write-Host "$tag $($step.Display)"
    if ($step.Gpu) { $env:PLEOCHROMA_REQUIRE_GPU = '1' }
    $sw = [Diagnostics.Stopwatch]::StartNew()
    Push-Location $Repo
    try {
        & $step.Command *> $log
        $code = $LASTEXITCODE
    } catch {
        $_ | Out-String | Add-Content -Path $log
        $code = -1
    } finally {
        Pop-Location
        Remove-Item Env:PLEOCHROMA_REQUIRE_GPU -ErrorAction SilentlyContinue
    }
    $sw.Stop()
    $minutes = [math]::Round($sw.Elapsed.TotalMinutes, 1)
    $verdict = if ($code -eq 0) { 'ok' } else { 'FAILED' }
    Write-Host "$tag $verdict ($minutes min)"
    Add-Content -Path $summary -Value ("| {0} | {1} | {2} | {3} | {4} | ``{5}`` |" -f $tag, $step.Name, $code, $verdict, $minutes, $step.Display)
}

Add-Content -Path $summary -Value @(
    '',
    'Not scripted (by hand): re-capture optimizer_2.png (manual 08 + root README) and the site figure',
    'fig_optimizer_result.png; send the new Fig. 21 numbers.'
)
Write-Host "Done. Summary: $summary"
