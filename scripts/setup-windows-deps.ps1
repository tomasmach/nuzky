$ErrorActionPreference = 'Stop'

# BtbN retains month-end builds for two years; daily pins disappear after 14 days.
$tag = 'autobuild-2026-09-30-13-08'
$name = 'ffmpeg-n8.1.3-9-g29e619e767-win64-gpl-shared-8.1'
$sha256 = 'dfe81c3aa0a546ee81980b1b824486dd0fe0a1d452f00cb55135379a74be7bcf'
$deps = Join-Path $env:LOCALAPPDATA 'Nuzky/build-deps'
New-Item -ItemType Directory -Force $deps | Out-Null
$archive = Join-Path $deps "$name.zip"
$url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/$tag/$name.zip"
if (!(Test-Path $archive)) {
    Invoke-WebRequest $url -OutFile $archive -TimeoutSec 120
}
if ((Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $sha256) {
    throw "FFmpeg checksum mismatch: delete $archive and retry."
}
$env:FFMPEG_DIR = Join-Path $deps $name
if (!(Test-Path "$env:FFMPEG_DIR/include/libavcodec/avcodec.h")) {
    Expand-Archive $archive -DestinationPath $deps -Force
}
if (!(Test-Path "$env:FFMPEG_DIR/lib/avcodec.lib")) {
    throw 'The FFmpeg archive must contain MSVC import libraries.'
}
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
if (!(Test-Path "$env:LIBCLANG_PATH/libclang.dll")) {
    throw 'Install LLVM x64 (winget install LLVM.LLVM), then rerun this script.'
}
if (!(Get-Command cmake -ErrorAction SilentlyContinue)) {
    throw 'Install CMake (winget install Kitware.CMake), then reopen the terminal.'
}
$env:Path = "$env:FFMPEG_DIR\bin;$env:LIBCLANG_PATH;$env:Path"
if ($env:GITHUB_ENV) {
    "FFMPEG_DIR=$env:FFMPEG_DIR" >> $env:GITHUB_ENV
    "LIBCLANG_PATH=$env:LIBCLANG_PATH" >> $env:GITHUB_ENV
    "$env:FFMPEG_DIR\bin" >> $env:GITHUB_PATH
    $env:LIBCLANG_PATH >> $env:GITHUB_PATH
}
& "$env:FFMPEG_DIR/bin/ffmpeg.exe" -version
if ($LASTEXITCODE -ne 0) { throw 'FFmpeg could not start.' }
