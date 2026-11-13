[CmdletBinding()]
param (
    [Parameter(Mandatory = $true, Position = 0, HelpMessage = "Path to the source MP4 file")]
    [ValidateScript({ Test-Path $_ -PathType Leaf })]
    [string]$VideoPath,

    [Parameter(Mandatory = $true, Position = 1, HelpMessage = "Path to the SRT subtitle file")]
    [ValidateScript({ Test-Path $_ -PathType Leaf })]
    [string]$SubtitlePath,

    [Parameter(Mandatory = $false, Position = 2, HelpMessage = "Optional custom output path")]
    [string]$OutputPath
)

$ErrorActionPreference = "Stop"

if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) {
    Write-Error "ffmpeg was not found in your PATH."
}

$fullVideoPath = (Resolve-Path -LiteralPath $VideoPath).Path
$fullSubtitlePath = (Resolve-Path -LiteralPath $SubtitlePath).Path

if (-not $OutputPath) {
    $directory = [System.IO.Path]::GetDirectoryName($fullVideoPath)
    $fileNameWithoutExt = [System.IO.Path]::GetFileNameWithoutExtension($fullVideoPath)
    $OutputPath = Join-Path -Path $directory -ChildPath "${fileNameWithoutExt}_muxed.mp4"
}

# FFmpeg filtergraphs require forward slashes and escaped colons on Windows
$escapedSubPath = $fullSubtitlePath.Replace('\', '/').Replace(':', '\:')

# Burns the subtitles into the original frame first, then rotates the entire composition 90° CCW
$filterGraph = "subtitles='$escapedSubPath',transpose=2"

$ffmpegArgs = @(
    "-hide_banner",
    "-i", $fullVideoPath,
    "-vf", $filterGraph,
    "-c:v", "libx264",
    "-crf", "18",
    "-preset", "fast",
    "-c:a", "copy",
    $OutputPath
)

& ffmpeg @ffmpegArgs

if ($LASTEXITCODE -eq 0) {
    Write-Host "Processing complete: $OutputPath" -ForegroundColor Green
}
else {
    Write-Error "ffmpeg exited with error code $LASTEXITCODE"
}