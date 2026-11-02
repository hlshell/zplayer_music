// ============================================================================
//  update.rs — 桌面端在线更新（web 不支持；移动端待定，未启用）
//
//  流程：check_latest() 查询 GitHub 最新 Release → 与编译期版本比较 →
//        download() 流式下载对应平台资产（带进度回调）→
//        apply() 解压到暂存目录 → 运行中文件改名 .old → 新文件落位 →
//        spawn_new_process() 启动新进程并退出当前进程。
//  下次启动时由 cleanup_old() 清理上次更新的 .old 残留。
//
//  平台资产映射（与 .github/workflows/release.yml 产物一致）：
//    windows/x86_64 → zplayer-music-windows-x64.zip   （单 exe，FFmpeg 静态内嵌）
//    linux/x86_64   → zplayer-music-linux-x64.tar.gz  （zplayer-music + lib/）
//    macos/x86_64   → zplayer-music-macos-x64.tar.gz  （Intel）
//    macos/aarch64  → zplayer-music-macos-arm64.tar.gz（Apple Silicon M 系列）
// ============================================================================

use std::path::{Path, PathBuf};

/// GitHub 仓库（owner/repo）
const REPO: &str = "hlshell/zplayer_music";

/// 一次可用更新
#[derive(Clone, Debug)]
pub struct UpdateInfo {
    /// 新版本号（去掉 v 前缀，如 "0.3.0"）
    pub version: String,
    /// Release 说明（markdown 原文，弹窗展示）
    pub notes: String,
    /// 当前平台对应资产的下载地址
    pub asset_url: String,
    /// 资产大小（字节；用于无 content-length 时的进度兜底显示）
    pub size: u64,
}

/// 检查结果
pub enum CheckResult {
    UpToDate,
    Available(UpdateInfo),
}

/// 当前平台对应的 Release 资产文件名；不支持的平台返回 None
fn asset_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("zplayer-music-windows-x64.zip"),
        ("linux", "x86_64") => Some("zplayer-music-linux-x64.tar.gz"),
        ("macos", "x86_64") => Some("zplayer-music-macos-x64.tar.gz"),
        // Apple Silicon（M 系列）：macOS aarch64 → arm64 资产
        ("macos", "aarch64") => Some("zplayer-music-macos-arm64.tar.gz"),
        _ => None,
    }
}

/// 查询最新 Release 并与当前版本比较
pub async fn check_latest() -> Result<CheckResult, String> {
    let want = asset_name().ok_or_else(|| "当前平台不支持在线更新".to_string())?;
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let client = crate::network::proxy::build_client(Some(std::time::Duration::from_secs(15)));
    let resp = client
        .get(&url)
        // GitHub API 要求 UA，否则 403
        .header("User-Agent", concat!("zplayer-music/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API 返回 {}", resp.status()));
    }
    let json: serde_json::Value = resp.json().await.map_err(|e| format!("解析响应失败: {e}"))?;
    let tag = json.get("tag_name").and_then(|v| v.as_str()).unwrap_or_default();
    let latest = semver::Version::parse(tag.trim_start_matches('v'))
        .map_err(|e| format!("版本号解析失败({tag}): {e}"))?;
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|e| format!("当前版本解析失败: {e}"))?;
    if latest <= current {
        return Ok(CheckResult::UpToDate);
    }
    let notes = json.get("body").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let asset = json
        .get("assets")
        .and_then(|v| v.as_array())
        .and_then(|a| a.iter().find(|x| x.get("name").and_then(|n| n.as_str()) == Some(want)))
        .ok_or_else(|| format!("Release 中缺少当前平台资产 {want}"))?;
    let asset_url = asset
        .get("browser_download_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "资产下载地址缺失".to_string())?
        .to_string();
    let size = asset.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
    Ok(CheckResult::Available(UpdateInfo {
        version: latest.to_string(),
        notes,
        asset_url,
        size,
    }))
}

/// 流式下载资产到临时文件（阻塞式，调用方应放进 spawn_blocking）；
/// progress(已下载字节, 总字节) 在下载线程回调（非 UI 线程）
pub fn download(info: &UpdateInfo, progress: impl Fn(u64, u64)) -> Result<PathBuf, String> {
    use std::io::{Read, Write};

    let client = crate::network::proxy::build_download_client_blocking();
    let mut resp = client
        .get(&info.asset_url)
        .header("User-Agent", concat!("zplayer-music/", env!("CARGO_PKG_VERSION")))
        .send()
        .map_err(|e| format!("下载请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败: HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(info.size);
    let tmp = std::env::temp_dir().join(format!(
        "zplayer-update-{}-{}{}",
        info.version,
        std::process::id(),
        if cfg!(windows) { ".zip" } else { ".tar.gz" }
    ));
    let mut file = std::fs::File::create(&tmp).map_err(|e| format!("创建临时文件失败: {e}"))?;
    let mut got: u64 = 0;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = resp.read(&mut buf).map_err(|e| format!("下载中断: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("写入临时文件失败: {e}"))?;
        got += n as u64;
        progress(got, total);
    }
    Ok(tmp)
}

/// 当前 exe 同目录的 lib/（Linux/macOS 发行包内含 FFmpeg 动态库）
fn lib_dir_of(exe: &Path) -> PathBuf {
    exe.parent().map(|p| p.join("lib")).unwrap_or_default()
}

/// 追加 .old 后缀（保留原文件名整体，如 "ZPlayer Music.exe.old"）
fn old_path(p: &Path) -> PathBuf {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    p.with_file_name(format!("{name}.old"))
}

/// 带重试的改名：杀毒软件/索引服务可能短暂占用文件句柄
fn rename_retry(from: &Path, to: &Path) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..5 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = e.to_string();
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }
    Err(format!("改名 {} 失败: {last}", from.display()))
}

/// 跨卷移动文件：rename 仅限同卷；暂存目录（系统 temp 盘）与程序目录可能不同盘，
/// 失败时回退 copy + 删除源
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(|e| format!("写入 {} 失败: {e}", to.display()))?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

/// 跨卷移动目录（递归 copy + 删除源）
fn move_dir(from: &Path, to: &Path) -> Result<(), String> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_dir(from, to)?;
    let _ = std::fs::remove_dir_all(from);
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("创建目录 {} 失败: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("读取目录失败: {e}"))? {
        let entry = entry.map_err(|e| format!("读取目录项失败: {e}"))?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            move_file(&src, &dst)?;
        }
    }
    Ok(())
}

/// 解压资产并替换当前程序文件（不重启）。同步阻塞，调用方应放进 spawn_blocking。
/// 成功后当前进程镜像已改名 .old，原路径已是新版本。
pub fn apply(pkg: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("定位当前程序失败: {e}"))?;
    let stage = std::env::temp_dir().join(format!("zplayer-update-stage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| format!("创建暂存目录失败: {e}"))?;

    // 1) 解压到暂存目录
    if cfg!(windows) {
        let f = std::fs::File::open(pkg).map_err(|e| format!("打开更新包失败: {e}"))?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| format!("更新包解压失败: {e}"))?;
        z.extract(&stage).map_err(|e| format!("更新包解压失败: {e}"))?;
    } else {
        let f = std::fs::File::open(pkg).map_err(|e| format!("打开更新包失败: {e}"))?;
        let gz = flate2::read::GzDecoder::new(f);
        let mut ar = tar::Archive::new(gz);
        ar.unpack(&stage).map_err(|e| format!("更新包解压失败: {e}"))?;
    }

    // 2) 找到暂存目录中的新主程序（zip/tar 内文件名固定为 zplayer-music[.exe]）
    let new_exe_name = if cfg!(windows) { "zplayer-music.exe" } else { "zplayer-music" };
    let new_exe = stage.join(new_exe_name);
    if !new_exe.is_file() {
        return Err(format!("更新包内未找到 {new_exe_name}"));
    }

    // 3) 运行中文件改名 .old（Windows 允许改名运行中的 exe；Unix 允许改名/覆盖
    //    但运行中的 exe 直接写会 ETXTBSY，统一走改名 + 新文件落位）
    let exe_old = old_path(&exe);
    let _ = std::fs::remove_file(&exe_old); // 上次残留
    rename_retry(&exe, &exe_old)?;
    if let Err(e) = move_file(&new_exe, &exe) {
        // 失败回滚：恢复原 exe
        let _ = std::fs::rename(&exe_old, &exe);
        return Err(e);
    }

    // 4) lib/ 目录整体替换（Linux/macOS；Windows 无 lib，FFmpeg 已静态内嵌）
    let lib_new = stage.join("lib");
    if lib_new.is_dir() {
        let lib = lib_dir_of(&exe);
        let lib_old = old_path(&lib);
        let _ = std::fs::remove_dir_all(&lib_old);
        if lib.is_dir() {
            rename_retry(&lib, &lib_old)?;
        }
        move_dir(&lib_new, &lib)?;
    }

    // 5) Unix 需要可执行权限
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&exe) {
            let mut perm = meta.permissions();
            perm.set_mode(0o755);
            let _ = std::fs::set_permissions(&exe, perm);
        }
    }

    let _ = std::fs::remove_dir_all(&stage);
    let _ = std::fs::remove_file(pkg);
    log::info!("[update] 更新文件已落位: {}", exe.display());
    Ok(())
}

/// 启动新版本并退出当前进程（先落盘状态再由调用方退出事件循环）
pub fn spawn_new_process() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("定位当前程序失败: {e}"))?;
    std::process::Command::new(&exe)
        .spawn()
        .map_err(|e| format!("启动新版本失败: {e}"))?;
    log::info!("[update] 已启动新版本进程，当前进程即将退出");
    Ok(())
}

/// 启动时清理上次更新残留的 .old 文件/目录（尽力而为，失败忽略）
pub fn cleanup_old() {
    let Ok(exe) = std::env::current_exe() else { return };
    let exe_old = old_path(&exe);
    // 当前进程若正从 .old 运行（用户选了稍后重启后又手动启动了新实例），
    // 删除会失败——忽略错误即可，下次启动再清
    let _ = std::fs::remove_file(&exe_old);
    let lib_old = old_path(&lib_dir_of(&exe));
    if lib_old.is_dir() {
        let _ = std::fs::remove_dir_all(&lib_old);
    }
}
