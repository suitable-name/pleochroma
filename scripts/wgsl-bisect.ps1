# PowerShell twin of scripts/wgsl-bisect.sh: rebuilds the shaders with one
# `PLEOCHROMA_WGSL_BISECT` revert at a time (see `BISECT_PATCHES` in crates/pleochroma/build.rs)
# and runs the GPU equivalence harness for each.
#
# Usage (from anywhere inside the repository, on the machine with the GPU):
#   .\scripts\wgsl-bisect.ps1                          # baseline, every default key alone, then all
#   .\scripts\wgsl-bisect.ps1 nee_hg_off nee_carry_off # only these keys (each alone), plus baseline and all
#
# Writes target\wgsl-bisect\summary.txt: per variant the build warning that lists the reverted
# sites, the uniaxial / event-wavefront / NEE furnace lines and the harness verdict. Full logs
# next to it.

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot
$outDir = Join-Path $repoRoot "target\wgsl-bisect"
New-Item -ItemType Directory -Force $outDir | Out-Null
$summary = Join-Path $outDir "summary.txt"

$keys = @($args)
if ($keys.Count -eq 0) {
    $keys = @("ori_zoned", "bragg", "oriented")
}
$variants = @("") + $keys + @("all")

@(
    "wgsl-bisect summary"
    "date: $((Get-Date).ToUniversalTime().ToString('yyyy-MM-dd HH:mm:ss')) UTC"
    "host: $([System.Environment]::OSVersion.VersionString)"
    ""
) | Set-Content -Encoding utf8 $summary

$checkPattern = '^\[(Tier 3|Specialisation|Event Wavefront)\].*(Zircon|Tourmaline|Quartz|Rutile|Moissanite|Ruby|Star Silk|zoned dichroic|class 2)'

foreach ($variant in $variants) {
    $name = if ($variant -eq "") { "baseline" } else { $variant }
    $log = Join-Path $outDir "$name.log"
    Write-Host ">>> $name"
    $env:PLEOCHROMA_WGSL_BISECT = $variant
    $env:PLEOCHROMA_REQUIRE_GPU = "1"
    & cargo run --profile probe -p pleochroma --features gpu --example gpu_equivalence_harness *> $log
    $code = $LASTEXITCODE
    $lines = Get-Content $log

    $out = @(
        "=================================================================="
        "variant: $name (exit $code, full log: target/wgsl-bisect/$name.log)"
    )
    $out += $lines | Where-Object { $_ -match 'WGSL bisect:' } |
        ForEach-Object { $_ -replace '^warning: [^:]*: ', '' } | Sort-Object -Unique
    $out += $lines | Where-Object { $_ -match $checkPattern }
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^\[Furnace, NEE scattering\]') {
            $out += $lines[$i..([Math]::Min($i + 4, $lines.Count - 1))]
        }
    }
    $out += $lines | Where-Object { $_ -match '^gpu_equivalence_harness: (ALL|FAILED)|^error' } |
        ForEach-Object { if ($_.Length -gt 200) { $_.Substring(0, 200) } else { $_ } }
    $out += ""
    $out | Add-Content -Encoding utf8 $summary
}

Remove-Item Env:PLEOCHROMA_WGSL_BISECT -ErrorAction SilentlyContinue
Write-Host ""
Write-Host "summary: $summary"
