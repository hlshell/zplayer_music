// ============================================================================
//  autolaunch.rs — 开机自启（跨平台）
//   Windows: HKCU\Software\Microsoft\Windows\CurrentVersion\Run（reg 命令）
//   macOS:   ~/Library/LaunchAgents/com.zplayermusic.agent.plist + launchctl
//   Linux:   ~/.config/autostart/zplayermusic.desktop
//  失败不抛错：由调用方记录日志（自启为尽力而为，不应阻塞启动流程）。
// ============================================================================
use std::path::PathBuf;

fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_default()
}

/// 根据当前平台设置或取消开机自启。成功返回 Ok(true)（已按目标状态操作）。
pub fn set(enabled: bool) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        windows_set(enabled)
    }
    #[cfg(target_os = "macos")]
    {
        macos_set(enabled)
    }
    #[cfg(target_os = "linux")]
    {
        linux_set(enabled)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Ok(())
    }
}

// ---- Windows：注册表 Run 键（Reg API 直写，不 spawn reg.exe —— GUI 程序
//      调命令行子进程会弹出 console 窗口一闪而过，且子进程创建拖慢启动）----
#[cfg(target_os = "windows")]
fn windows_set(enabled: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    const HKEY_CURRENT_USER: isize = 0x8000_0001;
    const KEY_SET_VALUE: u32 = 0x0002;
    const REG_SZ: u32 = 1;
    const ERROR_SUCCESS: i32 = 0;
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const SUB_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const NAME: &str = "ZPlayerMusic";

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegOpenKeyExW(hkey: isize, subkey: *const u16, options: u32, access: u32, out: *mut isize) -> i32;
        fn RegSetValueExW(hkey: isize, name: *const u16, reserved: u32, typ: u32, data: *const u8, len: u32) -> i32;
        fn RegDeleteValueW(hkey: isize, name: *const u16) -> i32;
        fn RegCloseKey(hkey: isize) -> i32;
    }
    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }
    unsafe {
        let mut hkey: isize = 0;
        let rc = RegOpenKeyExW(HKEY_CURRENT_USER, wide(SUB_KEY).as_ptr(), 0, KEY_SET_VALUE, &mut hkey);
        if rc != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(rc));
        }
        let name_w = wide(NAME);
        let result = if enabled {
            // REG_SZ 数据需含结尾 null 的字节长度
            let value = format!("\"{}\"", exe_path().display());
            let mut data = wide(&value);
            RegSetValueExW(
                hkey, name_w.as_ptr(), 0, REG_SZ,
                data.as_mut_ptr() as *const u8,
                (data.len() * 2) as u32,
            )
        } else {
            // 键值本就不存在视为已达成目标状态
            let rc = RegDeleteValueW(hkey, name_w.as_ptr());
            if rc == ERROR_FILE_NOT_FOUND { ERROR_SUCCESS } else { rc }
        };
        RegCloseKey(hkey);
        if result != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(result));
        }
    }
    Ok(())
}

// ---- macOS：LaunchAgent plist + launchctl ----
#[cfg(target_os = "macos")]
fn macos_set(enabled: bool) -> std::io::Result<()> {
    use std::process::Command;
    let Some(home) = dirs::home_dir() else { return Ok(()) };
    let agents = home.join("Library/LaunchAgents");
    let plist = agents.join("com.zplayermusic.agent.plist");
    if enabled {
        std::fs::create_dir_all(&agents)?;
        let exe = exe_path().display().to_string();
        // plist XML 中需转义 & < >
        let exe_esc = exe.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let content = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>\n\
             <key>Label</key><string>com.zplayermusic.agent</string>\n\
             <key>ProgramArguments</key><array><string>{}</string></array>\n\
             <key>RunAtLoad</key><true/>\n\
             </dict></plist>\n",
            exe_esc
        );
        std::fs::write(&plist, content)?;
        let _ = Command::new("launchctl")
            .args(["load", "-w", plist.to_str().unwrap_or_default()])
            .status();
    } else {
        let _ = Command::new("launchctl")
            .args(["unload", "-w", plist.to_str().unwrap_or_default()])
            .status();
        let _ = std::fs::remove_file(&plist);
    }
    Ok(())
}

// ---- Linux：XDG autostart .desktop ----
#[cfg(target_os = "linux")]
fn linux_set(enabled: bool) -> std::io::Result<()> {
    let Some(home) = dirs::home_dir() else { return Ok(()) };
    let cfg = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let autostart = cfg.join("autostart");
    let file = autostart.join("zplayermusic.desktop");
    if enabled {
        std::fs::create_dir_all(&autostart)?;
        let content = format!(
            "[Desktop Entry]\nType=Application\nName=ZPlayer Music\nExec=\"{}\"\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
            exe_path().display()
        );
        std::fs::write(&file, content)?;
    } else {
        let _ = std::fs::remove_file(&file);
    }
    Ok(())
}
