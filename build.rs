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
}
