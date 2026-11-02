# 构建发布版
# 任务管理器「进程」页显示名 = exe 内嵌 VERSIONINFO 的 FileDescription
# （resources/app.rc），无需复制改名 exe；「详细信息」页恒显示裸文件名。
# 用法：powershell -NoProfile -ExecutionPolicy Bypass -File build.ps1 [-Run]
# FFmpeg 静态链接（Cargo feature static）：产物为单个 exe，无需附带 DLL。
param([switch]$Run)

# 设置 vcpkg ffmpeg 查找环境变量（ffmpeg-sys-next 通过 vcpkg crate + pkg-config 定位静态库）
# 使用 x64-windows-static（/MT 静态 CRT），配合 Rust 的 +crt-static，exe 不依赖任何 VC++ 运行库
$env:VCPKG_ROOT = "C:\vcpkg"
$env:VCPKG_TARGET_TRIPLET = "x64-windows-static"
$env:PKG_CONFIG_PATH = "C:\vcpkg\installed\x64-windows-static\lib\pkgconfig"
$env:RUSTFLAGS = "-C target-feature=+crt-static"

# bindgen 需要的 clang/llvm + MSVC/Windows SDK 头文件路径
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
$ucrt = "C:\Program Files (x86)\Windows Kits\10\Include\10.0.26100.0\ucrt"
$msvc = "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.44.35207\include"
$env:BINDGEN_EXTRA_CLANG_ARGS = "-I`"$ucrt`" -I`"$msvc`""

$env:PATH = "C:\Program Files\LLVM\bin;C:\vcpkg\installed\x64-windows\bin;C:\vcpkg\installed\x64-windows\tools\pkgconf;C:\vcpkg\downloads\tools\msys2\d5586eb050429df1\mingw64\bin;$env:PATH"

# vcpkg 静态库是 MSVC 格式（avdevice.lib 等），GNU 工具链（windows-gnu）只查找
# libavdevice.a 会报「could not find native static library」，因此强制使用 MSVC 工具链。
cargo +stable-x86_64-pc-windows-msvc build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host "OK: target\release\zplayer-music.exe（单 exe，FFmpeg 已静态链接）"

if ($Run) {
    Start-Process "target\release\zplayer-music.exe" -WorkingDirectory (Get-Location)
}
