# Downloads the pinned FFmpeg build the desktop engine links against into third_party/ffmpeg.
#
# The build is the LGPL "shared" variant from BtbN/FFmpeg-Builds (the Windows builds linked from
# ffmpeg.org), so PairCam links to it dynamically and stays MIT. The archive is verified against
# a pinned SHA-256 before it is unpacked.
#
#   powershell -ExecutionPolicy Bypass -File scripts/fetch-ffmpeg.ps1

$ErrorActionPreference = "Stop"

$tag = "autobuild-2026-11-10-12-39"
$archive = "ffmpeg-n8.1.3-34-ga35157abb9-win64-lgpl-shared-8.1.zip"
$sha256 = "3045c2213ed8cbb05edf0d5a631b4490061de3b7567ec145644156e7c98da2c6"
$url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/$tag/$archive"

$root = Split-Path -Parent $PSScriptRoot
$thirdParty = Join-Path $root "third_party"
$target = Join-Path $thirdParty "ffmpeg"
$stamp = Join-Path $target ".version"

if ((Test-Path $stamp) -and ((Get-Content $stamp -Raw).Trim() -eq $sha256)) {
    Write-Output "FFmpeg is already up to date in $target"
    exit 0
}

New-Item -ItemType Directory -Force $thirdParty | Out-Null
$download = Join-Path $thirdParty $archive

Write-Output "Downloading $archive"
$ProgressPreference = "SilentlyContinue"
Invoke-WebRequest -Uri $url -OutFile $download

$actual = (Get-FileHash -Algorithm SHA256 $download).Hash.ToLowerInvariant()
if ($actual -ne $sha256) {
    Remove-Item $download
    throw "Checksum mismatch for $archive. Expected $sha256 but got $actual."
}

if (Test-Path $target) {
    Remove-Item -Recurse -Force $target
}
$unpacked = Join-Path $thirdParty "ffmpeg-unpack"
if (Test-Path $unpacked) {
    Remove-Item -Recurse -Force $unpacked
}
Expand-Archive -Path $download -DestinationPath $unpacked
$inner = Get-ChildItem $unpacked -Directory | Select-Object -First 1
Move-Item $inner.FullName $target
Remove-Item -Recurse -Force $unpacked
Remove-Item $download

Set-Content -Path $stamp -Value $sha256
Write-Output "FFmpeg unpacked to $target"
