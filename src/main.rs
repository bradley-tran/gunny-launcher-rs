#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Deserialize;
use std::io::Read;
use std::path::PathBuf;
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop, EventLoopBuilder},
    window::WindowBuilder,
};

// ── Custom events ─────────────────────────────────────────────────────────────

#[derive(Debug)]
enum AppEvent {
    /// Sent when Ruffle has been successfully launched; causes the window to close.
    Quit,
}
use wry::WebViewBuilder;

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Clone)]
#[serde(default)]
struct Config {
    launcher: LauncherConfig,
    ruffle: RuffleConfig,
}

#[derive(Deserialize, Clone)]
#[serde(default)]
struct LauncherConfig {
    login_url: String,
    window_title: String,
    window_width: f64,
    window_height: f64,
}

#[derive(Deserialize, Clone)]
#[serde(default)]
struct RuffleConfig {
    binary: String,
    scale: String,
    force_scale: bool,
    /// Stage alignment: "" = centered, "L" = left, "TL" = top-left, etc.
    align: String,
    /// Lock the alignment so the SWF cannot override it.
    force_align: bool,
    /// How to handle TCP socket connections: "allow", "deny", or "ask".
    tcp_connections: String,
    /// Extra endpoints to whitelist (e.g. ["127.0.0.1:5840"]).
    socket_allow: Vec<String>,
    /// Flash rendering quality: "low", "medium", "high", or "best".
    quality: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            launcher: LauncherConfig::default(),
            ruffle: RuffleConfig::default(),
        }
    }
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            login_url: "https://id-levelup.gn.zing.vn/login-game?channel=0".into(),
            window_title: "Gunny — Login".into(),
            window_width: 1280.0,
            window_height: 720.0,
        }
    }
}

impl Default for RuffleConfig {
    fn default() -> Self {
        Self {
            binary: "ruffle".into(),
            scale: "show-all".into(),
            force_scale: true,
            align: String::new(), // empty = centered
            force_align: true,
            tcp_connections: "allow".into(),
            socket_allow: Vec::new(),
            quality: "best".into(),
        }
    }
}

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gunny-launcher")
}

fn load_config() -> Config {
    let path = config_dir().join("config.toml");
    match std::fs::read_to_string(&path) {
        Ok(content) => match toml::from_str(&content) {
            Ok(config) => {
                println!("[config] loaded from {}", path.display());
                config
            }
            Err(e) => {
                eprintln!("[config] parse error: {e}, using defaults");
                Config::default()
            }
        },
        Err(_) => {
            println!("[config] no config at {}, using defaults", path.display());
            Config::default()
        }
    }
}

// ── Ruffle availability check ────────────────────────────────────────────────

/// Ensures Ruffle is available, downloading it if necessary.
/// Returns false if Ruffle could not be obtained; true otherwise.
fn ensure_ruffle_available(ruffle_binary: &str) -> bool {
    let resolved = resolve_ruffle_binary(ruffle_binary);
    
    // Check if Ruffle already exists
    if resolved.exists() {
        println!("[startup] Ruffle found at: {}", resolved.display());
        return true;
    }
    
    println!("[startup] Ruffle not found, downloading...");
    download_ruffle();
    
    // Check again after download
    if resolved.exists() {
        println!("[startup] Ruffle downloaded successfully");
        return true;
    }
    
    eprintln!("[startup] Failed to obtain Ruffle");
    false
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let debug_mode = args.iter().any(|a| a == "--debug");
    
    if args.iter().any(|a| a == "--download-ruffle") {
        download_ruffle();
        std::process::exit(0);
    }

    let config = load_config();
    
    if debug_mode {
        println!("[startup] Debug mode enabled");
    }
    
    // Ensure Ruffle is available before launching the webview
    if !ensure_ruffle_available(&config.ruffle.binary) {
        eprintln!("[startup] Cannot proceed without Ruffle");
        eprintln!("[startup] Run with --download-ruffle to manually download, or install from https://ruffle.rs/downloads");
        std::process::exit(1);
    }

    let event_loop: EventLoop<AppEvent> = EventLoopBuilder::<AppEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title(&config.launcher.window_title)
        .with_inner_size(tao::dpi::LogicalSize::new(
            config.launcher.window_width,
            config.launcher.window_height,
        ))
        .build(&event_loop)
        .expect("failed to build window");

    let ruffle_cfg = config.ruffle.clone();

    // Persistent data directory for cookies, localStorage, etc.
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("gunny-launcher");
    let _ = std::fs::create_dir_all(&data_dir);
    println!("[data] web data at {}", data_dir.display());
    let mut web_context = wry::WebContext::new(Some(data_dir));

    let builder = WebViewBuilder::new_with_web_context(&mut web_context)
        .with_url(&config.launcher.login_url)
        .with_navigation_handler(move |url_str: String| {
            let parsed = match url::Url::parse(&url_str) {
                Ok(u) => u,
                Err(_) => return true,
            };

            let scheme = parsed.scheme();
            if matches!(scheme, "http" | "https" | "data" | "blob" | "about") {
                return true;
            }

            println!("[protocol] intercepted: {url_str}");
            let game_url = extract_game_url(&parsed);
            println!("[protocol] game page → {game_url}");

            let rc = ruffle_cfg.clone();
            let proxy = proxy.clone();
            std::thread::spawn(move || {
                launch_game(&game_url, &rc, proxy, debug_mode);
            });

            false
        });

    // On Linux, use build_gtk() to avoid UnsupportedWindowHandle on Wayland.
    #[cfg(target_os = "linux")]
    let _webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        builder
            .build_gtk(window.default_vbox().expect("no vbox"))
            .expect("failed to build webview")
    };

    #[cfg(not(target_os = "linux"))]
    let _webview = builder.build(&window).expect("failed to build webview");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(AppEvent::Quit) => {
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

// ── Game launcher ─────────────────────────────────────────────────────────────

fn launch_game(game_page_url: &str, ruffle_cfg: &RuffleConfig, proxy: tao::event_loop::EventLoopProxy<AppEvent>, debug_mode: bool) {
    println!("[game] fetching page…");

    let html = match ureq::get(game_page_url).call() {
        Ok(resp) => match resp.into_string() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[game] read error: {e}");
                return;
            }
        },
        Err(e) => {
            eprintln!("[game] fetch error: {e}");
            return;
        }
    };

    let swf_info = extract_swf_info(&html);
    if swf_info.url.is_empty() {
        eprintln!("[game] no SWF URL found in page");
        let preview: String = html.chars().take(3000).collect();
        eprintln!("[game] HTML preview:\n{preview}");
        return;
    }

    println!("[game] SWF → {}", swf_info.url);
    if let Some((w, h)) = swf_info.dimensions {
        println!("[game] dimensions → {w}×{h}");
    }

    let ruffle_bin = resolve_ruffle_binary(&ruffle_cfg.binary);
    println!("[game] ruffle binary: {}", ruffle_bin.display());
    let mut cmd = std::process::Command::new(&ruffle_bin);
    // Force X11 (XWayland) so that window resize requests are honored exactly.
    // On native Wayland, request_inner_size() is advisory and gets ignored,
    // so --width/--height have no effect. Unsetting WAYLAND_DISPLAY forces
    // winit to fall back to X11, and GDK_BACKEND=x11 ensures GTK agrees.
    cmd.env_remove("WAYLAND_DISPLAY")
       .env("GDK_BACKEND", "x11")
       .env("WINIT_UNIX_BACKEND", "x11")
       .env("WINIT_X11_SCALE_FACTOR", "1");
    cmd.arg("--scale").arg(&ruffle_cfg.scale);
    if ruffle_cfg.force_scale {
        cmd.arg("--force-scale");
    }
    if !ruffle_cfg.align.is_empty() {
        cmd.arg("--align").arg(&ruffle_cfg.align);
    }
    if ruffle_cfg.force_align {
        cmd.arg("--force-align");
    }
    if !ruffle_cfg.tcp_connections.is_empty() {
        cmd.arg("--tcp-connections").arg(&ruffle_cfg.tcp_connections);
    }
    for host in &ruffle_cfg.socket_allow {
        cmd.arg("--socket-allow").arg(host);
    }
    if !ruffle_cfg.quality.is_empty() {
        cmd.arg("--quality").arg(&ruffle_cfg.quality);
    }
    
    // Hide the Ruffle GUI by default; show it if --debug is passed.
    if !debug_mode {
        cmd.arg("--no-gui");
    }
    
    cmd.arg(&swf_info.url);

    println!("[game] launching: {} …", ruffle_bin.display());

    match cmd.spawn() {
        Ok(child) => {
            println!("[game] launched (pid {})", child.id());
            let _ = proxy.send_event(AppEvent::Quit);
        }
        Err(e) => {
            eprintln!("[game] failed to launch '{}': {e}", ruffle_bin.display());
            eprintln!("[game] run with --download-ruffle to fetch Ruffle, or install from https://ruffle.rs/downloads");
        }
    }
}

// ── SWF extraction ────────────────────────────────────────────────────────────

struct SwfInfo {
    url: String,
    dimensions: Option<(u32, u32)>,
}

fn extract_swf_info(html: &str) -> SwfInfo {
    let lower = html.to_lowercase();

    // Strategy 1: <embed ... src="...swf..." width="..." height="...">
    let mut search_from = 0;
    while let Some(pos) = lower[search_from..].find("<embed") {
        let abs = search_from + pos;
        if let Some(close) = html[abs..].find('>') {
            let tag = &html[abs..abs + close];
            if let Some(src) = extract_attr(tag, "src") {
                let decoded = html_decode(&src);
                if decoded.contains(".swf") {
                    let dims = parse_dimensions(tag);
                    return SwfInfo { url: decoded, dimensions: dims };
                }
            }
        }
        search_from = abs + 6;
    }

    // Strategy 2: <param name="movie" value="...swf...">
    search_from = 0;
    while let Some(pos) = lower[search_from..].find("<param") {
        let abs = search_from + pos;
        if let Some(close) = html[abs..].find('>') {
            let tag = &html[abs..abs + close];
            let tag_lc = tag.to_lowercase();
            if tag_lc.contains("name=\"movie\"") || tag_lc.contains("name='movie'") {
                if let Some(val) = extract_attr(tag, "value") {
                    let decoded = html_decode(&val);
                    if decoded.contains(".swf") {
                        let dims = lower[..abs]
                            .rfind("<object")
                            .and_then(|obj_pos| {
                                html[obj_pos..].find('>').and_then(|c| {
                                    parse_dimensions(&html[obj_pos..obj_pos + c])
                                })
                            });
                        return SwfInfo { url: decoded, dimensions: dims };
                    }
                }
            }
        }
        search_from = abs + 6;
    }

    SwfInfo { url: String::new(), dimensions: None }
}

fn parse_dimensions(tag: &str) -> Option<(u32, u32)> {
    let w = extract_attr(tag, "width")?.parse::<u32>().ok()?;
    let h = extract_attr(tag, "height")?.parse::<u32>().ok()?;
    Some((w, h))
}

fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    for quote in ['"', '\''] {
        let pattern = format!("{attr}={quote}");
        if let Some(pos) = lower.find(&pattern) {
            let start = pos + pattern.len();
            if let Some(end) = tag[start..].find(quote) {
                return Some(tag[start..start + end].to_string());
            }
        }
    }
    None
}

fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

// ── Ruffle downloader ────────────────────────────────────────────────────────

fn download_ruffle() {
    println!("[ruffle-dl] fetching latest release info from GitHub...");
    let api_url = "https://api.github.com/repos/ruffle-rs/ruffle/releases?per_page=1";
    let json = match ureq::get(api_url).set("User-Agent", "gunny-launcher").call() {
        Ok(resp) => match resp.into_string() {
            Ok(s) => s,
            Err(e) => { eprintln!("[ruffle-dl] failed to read response: {e}"); return; }
        },
        Err(e) => { eprintln!("[ruffle-dl] failed to fetch release info: {e}"); return; }
    };

    #[cfg(target_os = "windows")]
    let platform_suffix = "windows-x86_64.zip";
    #[cfg(target_os = "linux")]
    let platform_suffix = "linux-x86_64.tar.gz";
    #[cfg(target_os = "macos")]
    let platform_suffix = "macos-universal.tar.gz";

    let download_url = match find_ruffle_url(&json, platform_suffix) {
        Some(u) => u,
        None => {
            eprintln!("[ruffle-dl] no asset found for platform suffix '{platform_suffix}'");
            return;
        }
    };

    println!("[ruffle-dl] downloading {download_url} ...");
    let resp = match ureq::get(&download_url).set("User-Agent", "gunny-launcher").call() {
        Ok(r) => r,
        Err(e) => { eprintln!("[ruffle-dl] download failed: {e}"); return; }
    };

    let mut data: Vec<u8> = Vec::new();
    if let Err(e) = resp.into_reader().read_to_end(&mut data) {
        eprintln!("[ruffle-dl] read error: {e}");
        return;
    }
    println!("[ruffle-dl] downloaded {} bytes", data.len());

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let dest = cwd.join("ruffle-bin");
    if let Err(e) = std::fs::create_dir_all(&dest) {
        eprintln!("[ruffle-dl] failed to create ruffle-bin dir: {e}");
        return;
    }
    println!("[ruffle-dl] extracting to {}...", dest.display());

    #[cfg(target_os = "windows")]
    extract_zip(&data, &dest);
    #[cfg(not(target_os = "windows"))]
    extract_tar_gz(&data, &dest);

    println!("[ruffle-dl] done.");
}

/// Scan the GitHub releases JSON for a `browser_download_url` ending with `suffix`.
fn find_ruffle_url(json: &str, suffix: &str) -> Option<String> {
    let key = "\"browser_download_url\":\"";
    let mut pos = 0;
    while let Some(found) = json[pos..].find(key) {
        let url_start = pos + found + key.len();
        if let Some(url_end) = json[url_start..].find('"') {
            let url = &json[url_start..url_start + url_end];
            if url.ends_with(suffix) {
                return Some(url.to_string());
            }
            pos = url_start + url_end;
        } else {
            break;
        }
    }
    None
}

/// Join `entry_name` onto `base`, ignoring any components that would escape it
/// (zip-slip prevention).
fn safe_join(base: &std::path::Path, entry_name: &str) -> Option<PathBuf> {
    use std::path::Component;
    let mut result = base.to_path_buf();
    for component in std::path::Path::new(entry_name).components() {
        match component {
            Component::Normal(part) => result.push(part),
            Component::CurDir => {}
            // Drop parent-dir, root, and prefix components to prevent escaping base.
            _ => {}
        }
    }
    if result.starts_with(base) { Some(result) } else { None }
}

#[cfg(target_os = "windows")]
fn extract_zip(data: &[u8], dest: &PathBuf) {
    use std::io::Cursor;
    let mut archive = match zip::ZipArchive::new(Cursor::new(data)) {
        Ok(a) => a,
        Err(e) => { eprintln!("[ruffle-dl] zip open error: {e}"); return; }
    };
    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(e) => { eprintln!("[ruffle-dl] zip entry error: {e}"); continue; }
        };
        let name = entry.name().to_owned();
        let outpath = match safe_join(dest, &name) {
            Some(p) => p,
            None => { eprintln!("[ruffle-dl] skipping unsafe path: {name}"); continue; }
        };
        if entry.is_dir() {
            let _ = std::fs::create_dir_all(&outpath);
        } else {
            if let Some(parent) = outpath.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut outfile = match std::fs::File::create(&outpath) {
                Ok(f) => f,
                Err(e) => { eprintln!("[ruffle-dl] create {}: {e}", outpath.display()); continue; }
            };
            match std::io::copy(&mut entry, &mut outfile) {
                Ok(_) => println!("[ruffle-dl] extracted: {}", outpath.display()),
                Err(e) => eprintln!("[ruffle-dl] write {}: {e}", outpath.display()),
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn extract_tar_gz(data: &[u8], dest: &PathBuf) {
    use std::io::Cursor;
    let gz = flate2::read::GzDecoder::new(Cursor::new(data));
    let mut archive = tar::Archive::new(gz);
    match archive.unpack(dest) {
        Ok(_) => println!("[ruffle-dl] extracted to {}", dest.display()),
        Err(e) => eprintln!("[ruffle-dl] tar extract error: {e}"),
    }
}

/// Resolve the ruffle binary: use the configured name if it's found in PATH,
/// otherwise fall back to `<cwd>/ruffle-bin/<name>`.
fn resolve_ruffle_binary(configured: &str) -> PathBuf {
    let p = PathBuf::from(configured);
    // Explicit path — use as-is.
    if p.is_absolute() || configured.contains('/') || configured.contains('\\') {
        return p;
    }
    if which_in_path(configured) {
        return p;
    }
    // Fall back to ruffle-bin/<name> relative to cwd.
    #[cfg(windows)]
    let fallback_name = if configured.ends_with(".exe") {
        configured.to_string()
    } else {
        format!("{configured}.exe")
    };
    #[cfg(not(windows))]
    let fallback_name = configured.to_string();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.join("ruffle-bin").join(fallback_name)
}

/// Returns true if `name` resolves to an executable file via PATH.
fn which_in_path(name: &str) -> bool {
    let path_var = match std::env::var("PATH") {
        Ok(v) => v,
        Err(_) => return false,
    };
    let sep = if cfg!(windows) { ';' } else { ':' };
    for dir in path_var.split(sep) {
        let candidate = PathBuf::from(dir).join(name);
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            if !name.ends_with(".exe") {
                let with_exe = PathBuf::from(dir).join(format!("{name}.exe"));
                if with_exe.is_file() {
                    return true;
                }
            }
        }
    }
    false
}

/// roadclient://https//host/path?query → https://host/path?query
fn extract_game_url(url: &url::Url) -> String {
    let scheme = url.host_str().unwrap_or("https");
    let path = url.path();
    let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
    format!("{scheme}:{path}{query}")
}
