#Requires -Version 7

<#
.SYNOPSIS
  Runs the Windows checks: the pins that the companion direction match changes, one GPU build
  of the changed kernels, the staged megakernel's tests and the sorted wavefront's tests, on this
  machine's default adapter (the AMD Radeon integrated GPU on Vulkan; set WGPU_BACKEND=dx12 first
  to test DX12).

.DESCRIPTION
  Runs each command from the repo root, keeps going when one fails, and writes:
    C:\temp\mcp2\owner_runs\windows\<timestamp>\<nn>-<name>.log   full output of each command
    C:\temp\mcp2\owner_runs\windows\<timestamp>\summary.md        exit code and duration per command
    C:\temp\mcp2\owner_runs\windows\LATEST.txt                    path of the newest run folder
  Nothing is re-pinned by this script: the printed values are read afterwards.

.PARAMETER Repo
  The repository root (default C:\Users\ngussek\Dev\Rust\diagram_loader).

.PARAMETER SkipGpu
  Skip the GPU steps (gpu-changed-kernels, staged-megakernel and sorted-wavefront), for a run of
  the CPU pins only.
#>
param(
    [string]$Repo = 'C:\Users\ngussek\Dev\Rust\diagram_loader',
    [switch]$SkipGpu
)

$ErrorActionPreference = 'Continue'
# The engine switches never come from the caller's shell: steps 3 to 5 compare against the
# production engine, the staged megakernel (the staged and sorted twins switch their engine per
# renderer). Run with `.\`, this also clears them in the calling session.
Remove-Item -ErrorAction SilentlyContinue -Path Env:PLEOCHROMA_SORTED, Env:PLEOCHROMA_STAGED,
    Env:PLEOCHROMA_GATED_LOOP, Env:PLEOCHROMA_GPU_PIPELINE
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

# The companion direction match (|a - b|^2 <= 2e-6) changes the traced samples of every dispersive
# stone. The cpu_accumulate pins hold Windows bits (ignored off Windows; Diamond and Zircon, both
# dispersive). The raytracer goldens are shared with Linux, so this run shows whether the Linux
# re-pin holds here. Step 3 builds the changed kernels on this machine's GPU: the production
# megakernel, its exit-collection variant on Diamond and Zircon, and the event wavefront's
# collection. The AMD driver has crashed on naga-valid WGSL with loop-indexed arrays before
# (collect_leaf_weight loops over an array parameter). No step checks the version: no test pins a
# version literal. Step 4 runs the staged megakernel's tests (the default engine, switched off
# with PLEOCHROMA_STAGED=0): its twins against the megakernel on Zircon and Diamond with the stage
# ends [3], [2, 5] and [1, 2, 3]. Its WGSL copies the path state through loop-indexed local arrays
# (spectral_transport_staged.wgsl), the pattern this driver has crashed on. Step 5 runs the sorted
# wavefront's tests (opt-in through PLEOCHROMA_SORTED): its twins against the megakernel, the
# exactly-once accounting, the exit collection and the decline, and the naga checks of its two
# units. Both run on this machine's default adapter (the AMD Radeon integrated GPU on Vulkan, whose
# pipeline cache keeps compiled kernels between runs; under WGPU_BACKEND=dx12 wgpu keeps none and
# every GPU test compiles its class's kernels). Step 3's --include-ignored also runs the
# exit-collection twin, which the workspace run ignores (it takes over 60 s).
$steps = @(
    @{ Name = 'cpu-accumulate-pins'; Gpu = $false
       Display = 'cargo test -p pleochroma --features gpu --lib renderer::gpu::pin_tests -- --nocapture'
       Command = { cargo test -p pleochroma --features gpu --lib renderer::gpu::pin_tests -- --nocapture } }
    @{ Name = 'raytracer-tests'; Gpu = $false
       Display = 'cargo test -p pleochroma --features gpu --test raytracer_tests -- --nocapture'
       Command = { cargo test -p pleochroma --features gpu --test raytracer_tests -- --nocapture } }
    @{ Name = 'gpu-changed-kernels'; Gpu = $true
       Display = 'cargo test -p pleochroma --features gpu --lib -- renderer::gpu_backend::tests renderer::gpu::frame::collect_hardware_tests renderer::gpu::event_wavefront::tests::ew_exit_collection_follows_the_megakernel --include-ignored --nocapture --test-threads 1'
       Command = { cargo test -p pleochroma --features gpu --lib -- renderer::gpu_backend::tests renderer::gpu::frame::collect_hardware_tests renderer::gpu::event_wavefront::tests::ew_exit_collection_follows_the_megakernel --include-ignored --nocapture --test-threads 1 } }
    @{ Name = 'staged-megakernel'; Gpu = $true
       Display = 'cargo test -p pleochroma --features gpu --lib -- renderer::gpu::frame::staged::tests --nocapture --test-threads 1'
       Command = { cargo test -p pleochroma --features gpu --lib -- renderer::gpu::frame::staged::tests --nocapture --test-threads 1 } }
    @{ Name = 'sorted-wavefront'; Gpu = $true
       Display = 'cargo test -p pleochroma --features gpu --lib -- renderer::gpu::frame::sorted_tests renderer::shader_validation_sorted_tests --include-ignored --nocapture --test-threads 1'
       Command = { cargo test -p pleochroma --features gpu --lib -- renderer::gpu::frame::sorted_tests renderer::shader_validation_sorted_tests --include-ignored --nocapture --test-threads 1 } }
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

Write-Host "Done. Summary: $summary"
