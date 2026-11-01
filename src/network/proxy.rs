// ============================================================================
//  net.rs — 全局网络配置（代理 / realip）
//  设置页在运行时修改代理与 realip 后，通过 update() 提升版本号，
//  netease 客户端在每次请求前检测版本变化并重建 reqwest Client，使新配置真实生效。
//  - 代理类型：0=HTTP 1=HTTPS 2=SOCKS4 3=SOCKS5
//    reqwest 的 socks feature 仅支持 SOCKS5（socks5://）；SOCKS4 按 SOCKS5 连接（多数
//    现代代理兼容），记录该限制。
//  - realip：把 API 域名 DNS 覆盖到指定 IP。realip_addr 支持两种形式：
//      * 纯 IP（如 1.2.3.4）：应用到内置 API 域名列表
//      * host=ip;host2=ip2（分号分隔多条）：自由指定
// ============================================================================
use reqwest::ClientBuilder;
use std::net::{IpAddr, SocketAddr};
use std::sync::{OnceLock, RwLock};

#[derive(Clone, Debug, Default)]
pub struct NetOptions {
    pub proxy_enabled: bool,
    pub proxy_type_index: i32,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub realip_enabled: bool,
    pub realip_addr: String,
}

/// 需要应用 realip DNS 覆盖的内置 API 域名（可扩展）
const API_DOMAINS: &[&str] = &[
    "music.163.com",
    "m.music.163.com",
    "interface.music.163.com",
    "p1.music.126.net",
    "p2.music.126.net",
    "p3.music.126.net",
];

struct State {
    version: u64,
    opts: NetOptions,
}

static NET: OnceLock<RwLock<State>> = OnceLock::new();

fn state() -> &'static RwLock<State> {
    NET.get_or_init(|| RwLock::new(State { version: 0, opts: NetOptions::default() }))
}

/// 启动时初始化（config → 全局网络配置）
pub fn init(opts: NetOptions) {
    let mut s = state().write().unwrap();
    s.version += 1;
    s.opts = opts;
}

/// 设置页运行时更新配置（提升版本号，触发客户端重建）
pub fn update(opts: NetOptions) {
    let mut s = state().write().unwrap();
    s.version += 1;
    s.opts = opts;
}

/// 当前配置版本号
pub fn current_version() -> u64 {
    state().read().unwrap().version
}

fn current_opts() -> NetOptions {
    state().read().unwrap().opts.clone()
}

/// 把当前代理/realip 配置应用到 ClientBuilder
pub fn apply(builder: ClientBuilder) -> ClientBuilder {
    let opts = current_opts();
    let mut b = builder;
    if opts.proxy_enabled && !opts.proxy_host.is_empty() && opts.proxy_port != 0 {
        let scheme = match opts.proxy_type_index {
            0 => "http",
            1 => "https",
            2 | 3 => "socks5", // reqwest 仅支持 socks5；SOCKS4 按 socks5 连接
            _ => "http",
        };
        let url = format!("{}://{}:{}", scheme, opts.proxy_host.trim(), opts.proxy_port);
        if let Ok(proxy) = reqwest::Proxy::all(url) {
            b = b.proxy(proxy);
        }
    }
    if opts.realip_enabled && !opts.realip_addr.trim().is_empty() {
        b = apply_realip(b, opts.realip_addr.trim());
    }
    b
}

fn apply_realip(builder: ClientBuilder, spec: &str) -> ClientBuilder {
    let mut b = builder;
    // 形式一：host=ip;host2=ip2（自由指定）
    if spec.contains('=') {
        for pair in spec.split(';').filter(|p| !p.is_empty()) {
            let mut it = pair.split('=');
            let (Some(host), Some(ip)) = (it.next(), it.next()) else { continue };
            if let Ok(addr) = ip.trim().parse::<IpAddr>() {
                b = b.resolve(host.trim(), SocketAddr::new(addr, 0));
            }
        }
        return b;
    }
    // 形式二：纯 IP → 应用到内置 API 域名
    if let Ok(addr) = spec.parse::<IpAddr>() {
        for domain in API_DOMAINS {
            b = b.resolve(domain, SocketAddr::new(addr, 0));
        }
    }
    b
}

/// 大文件下载专用 Client：连接超时 8s + 单次读超时 30s。
/// 不设"总超时"——限速较慢但仍在持续传输的下载不应被整体判死；
/// 真正要防的是服务端读挂起（此前无读超时，可无限卡在 bytes()）。
pub fn build_download_client() -> reqwest::Client {
    let b = reqwest::Client::builder()
        .cookie_store(true)
        .connect_timeout(std::time::Duration::from_secs(8))
        .read_timeout(std::time::Duration::from_secs(30));
    match apply(b).build() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[net] download client 构建失败: {e}");
            reqwest::Client::new()
        }
    }
}

/// 便捷：按当前配置构建一个全新 reqwest::Client（用于临时请求场景）
pub fn build_client(timeout: Option<std::time::Duration>) -> reqwest::Client {
    let mut b = reqwest::Client::builder()
        .cookie_store(true)
        .connect_timeout(std::time::Duration::from_secs(8));
    if let Some(t) = timeout {
        b = b.timeout(t);
    }
    match apply(b).build() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[net] client 构建失败: {e}");
            reqwest::Client::new()
        }
    }
}
