fn main() {
    // 使用 Fluent 组件样式
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    // web(wasm32)：编译 appwindow-web.slint wrapper（同一套 UI 骨架 + 额外编译期嵌入 SimHei
    // 中文字体，re-export AppWindow）；其余目标编译 appwindow.slint（系统字体，不嵌 9.7MB 字体）
    let ui_file = if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        "ui/appwindow-web.slint"
    } else {
        "ui/appwindow.slint"
    };
    slint_build::compile_with_config(ui_file, config)
        .expect("Slint UI 编译失败，请检查 appwindow.slint 语法");

    // Windows：用 winres 把 app.ico + 版本信息嵌入 exe 二进制
    // （任务管理器进程列表显示名优先取 FileDescription）
    // 注意：build script 按 host 编译，cfg(target_os="windows") 在 Windows host 上恒真；
    // 需再用 CARGO_CFG_TARGET_ARCH 判断真实编译目标，wasm(web) 不嵌入 winres 资源。
    #[cfg(target_os = "windows")]
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("wasm32") {
        let mut res = winres::WindowsResource::new();
        res.set_icon("ui/assets/app.ico");
        res.set("FileDescription", "ZPlayer Music");
        res.set("ProductName", "ZPlayer Music");
        res.set("FileVersion", "0.2.1");
        res.set("ProductVersion", "0.2.1");
        res.set("OriginalFilename", "zplayer-music.exe");
        res.compile().expect("嵌入 Windows 资源失败（winres）");
    }

    // Windows：FFmpeg 静态库（vcpkg 的 .lib）里 mediacodec 部分引用了 Media Foundation
    // 的 IID 符号，需显式链接 mfuuid（IID_IMFTransform 等）与 strmiids（IID_ICodecAPI）。
    #[cfg(target_os = "windows")]
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("wasm32") {
        println!("cargo:rustc-link-lib=mfuuid");
        println!("cargo:rustc-link-lib=strmiids");
    }

    // ---- Linux / macOS：静态链接 FFmpeg 时补足链接器参数 ----
    // build script 按 host 编译，必须用 CARGO_CFG_TARGET_OS 判断真实目标平台，
    // 否则在 Linux runner 上交叉编译 wasm32(web) 时会误加 ELF 链接参数。
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target_arch != "wasm32" {
        match target_os.as_str() {
            // GNU ld 对静态库顺序敏感（依赖者必须在使用者之后），而 ffmpeg-sys-next
            // 输出的 -l 顺序不保证成立；用 --start-group 循环解析，重复链接静态库无副作用。
            "linux" => {
                println!("cargo:rustc-link-arg=-Wl,--start-group");
                for lib in ["avformat", "avcodec", "swscale", "swresample", "avutil"] {
                    println!("cargo:rustc-link-arg=-l{lib}");
                }
                println!("cargo:rustc-link-arg=-Wl,--end-group");
                // 静态 libavcodec 依赖的系统库（Rust 默认已链部分，显式声明更稳妥）
                println!("cargo:rustc-link-arg=-lm");
                println!("cargo:rustc-link-arg=-lpthread");
                println!("cargo:rustc-link-arg=-ldl");
                // Slint/Skia 的 C++ 部分：静态化 libstdc++/libgcc，避免目标机
                // GLIBCXX 版本过低报 "GLIBCXX_x.x.x not found"
                println!("cargo:rustc-link-arg=-static-libstdc++");
                println!("cargo:rustc-link-arg=-static-libgcc");
            }
            // ld64 无 --start-group：按「依赖者在前」重复列出静态库即可（重复无害）。
            // 静态 libavcodec 的 videotoolbox 后端需要系统 framework，ffmpeg-sys-next 不会自动加。
            "macos" => {
                for lib in ["avformat", "avcodec", "swscale", "swresample", "avutil"] {
                    println!("cargo:rustc-link-arg=-l{lib}");
                }
                println!("cargo:rustc-link-arg=-lz");
                println!("cargo:rustc-link-arg=-lbz2");
                for fw in [
                    "VideoToolbox",
                    "AudioToolbox",
                    "CoreMedia",
                    "CoreVideo",
                    "CoreFoundation",
                    "CoreServices",
                    "Security",
                    "SystemConfiguration",
                ] {
                    println!("cargo:rustc-link-arg=-framework");
                    println!("cargo:rustc-link-arg={fw}");
                }
            }
            _ => {}
        }
    }
}
