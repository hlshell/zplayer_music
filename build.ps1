# 构建发布版
# 任务管理器「进程」页显示名 = exe 内嵌 VERSIONINFO 的 FileDescription
# （resources/app.rc），无需复制改名 exe；「详细信息」页恒显示裸文件名。
# 用法：powershell -NoProfile -ExecutionPolicy Bypass -File build.ps1 [-Run]
# ffmpeg DLL 部署：从 vcpkg 安装目录复制到 exe 同目录（avcodec-9.dll, avformat-9.dll 等）
param([switch]$Run)

# 设置 vcpkg ffmpeg 查找环境变量（ffmpeg-sys-next 通过 pkg-config 定位）
$env:VCPKG_ROOT = "C:\vcpkg"
$env:PKG_CONFIG_PATH = "C:\vcpkg\installed\x64-windows\lib\pkgconfig"

# bindgen 需要的 clang/llvm + MSVC/Windows SDK 头文件路径
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
$ucrt = "C:\Program Files (x86)\Windows Kits\10\Include\10.0.26100.0\ucrt"
$msvc = "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.44.35207\include"
$env:BINDGEN_EXTRA_CLANG_ARGS = "-I`"$ucrt`" -I`"$msvc`""

$env:PATH = "C:\Program Files\LLVM\bin;C:\vcpkg\installed\x64-windows\bin;C:\vcpkg\installed\x64-windows\tools\pkgconf;C:\vcpkg\downloads\tools\msys2\d5586eb050429df1\mingw64\bin;$env:PATH"

cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host "OK: target\release\zplayer-music.exe"

# 部署 ffmpeg 运行时 DLL（如果存在）
$ffmpegBin = "C:\vcpkg\installed\x64-windows\bin"
$targetExe = "target\release"
$ffmpegDlls = @("avcodec-63.dll", "avformat-63.dll", "avutil-61.dll", "swscale-10.dll", "swresample-7.dll", "avdevice-63.dll", "avfilter-12.dll")
$copied = 0
foreach ($dll in $ffmpegDlls) {
    $src = Join-Path $ffmpegBin $dll
    if (Test-Path $src) {
        Copy-Item -Path $src -Destination $targetExe -Force
        $copied++
    }
}
if ($copied -gt 0) {
    Write-Host "Deployed $copied ffmpeg DLL(s) to $targetExe"
} else {
    Write-Host "WARNING: no ffmpeg DLLs found in $ffmpegBin — run 'vcpkg install ffmpeg:x64-windows' first"
}

if ($Run) {
    Start-Process "target\release\zplayer-music.exe" -WorkingDirectory (Get-Location)
}
