//! VRDoctor — PCVR on Meta Quest 3 from Arch / Fedora Linux.
//!
//! Honest architecture: Meta does not provide a Linux Link/Air Link driver.
//! On Linux the working stacks are:
//!   * WiVRn  (recommended) — Linux-native, Monado-based OpenXR streamer.
//!              Native packages: AUR `wivrn-dashboard` (Arch), `wivrn` (Fedora),
//!              or Flatpak `io.github.wivrn.wivrn` (bundles xrizer/OpenComposite).
//!   * ALVR   — SteamVR streamer. Works for many users, but recent reports say
//!              its Linux build can break with newer SteamVR versions.
//!   * Steam Link — Valve's Quest app + SteamVR. Support on Linux is unofficial.
//!
//! VRDoctor does not re-implement video encoding / tracking. It:
//!   1. Detects your distro, GPU, Steam, runtimes, firewall and headset.
//!   2. Installs / points you to the right backend for Arch or Fedora.
//!   3. Starts the backend correctly, sets XR_RUNTIME_JSON / OpenVR overrides.
//!   4. Launches Steam games AND custom, non-store games (any executable,
//!      shell script, or Windows .exe via Proton/Wine) into that runtime.
//!   5. Provides a GUI (`vrdoctor gui`, dark mode by default) and a full CLI.

mod gui;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ---------------------------------------------------------------- distro

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Family { Arch, Fedora, Debian, Other }

#[derive(Debug, Clone)]
pub(crate) struct Distro { id: String, id_like: String, pretty: String, family: Family }

pub(crate) fn parse_os_release(content: &str) -> Distro {
    let mut kv = BTreeMap::new();
    for line in content.lines() {
        if let Some((k, v)) = line.split_once('=') {
            kv.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
        }
    }
    let id = kv.get("ID").cloned().unwrap_or_default().to_lowercase();
    let id_like = kv.get("ID_LIKE").cloned().unwrap_or_default().to_lowercase();
    let pretty = kv.get("PRETTY_NAME").cloned().unwrap_or_else(|| id.clone());
    let hay = format!("{id} {id_like}");
    let family = if hay.contains("arch") { Family::Arch }
        else if hay.contains("fedora") || hay.contains("rhel") || hay.contains("centos") { Family::Fedora }
        else if hay.contains("debian") || hay.contains("ubuntu") { Family::Debian }
        else { Family::Other };
    Distro { id, id_like, pretty, family }
}

pub(crate) fn current_distro() -> Distro {
    // VRDOCTOR_OS_RELEASE lets the test harness (scripts/test-distro.sh)
    // exercise the Arch and Fedora code paths on any machine.
    let path = env::var("VRDOCTOR_OS_RELEASE").unwrap_or_else(|_| "/etc/os-release".to_string());
    fs::read_to_string(path).map(|c| parse_os_release(&c))
        .unwrap_or(Distro { id: "unknown".into(), id_like: String::new(), pretty: "Unknown Linux".into(), family: Family::Other })
}

// ---------------------------------------------------------------- helpers

pub(crate) fn which(cmd: &str) -> Option<PathBuf> {
    if cmd.contains('/') {
        let p = PathBuf::from(cmd);
        return if p.exists() { Some(p) } else { None };
    }
    for dir in env::var_os("PATH")?.to_string_lossy().split(':').map(PathBuf::from).collect::<Vec<_>>() {
        let cand = dir.join(cmd);
        if cand.is_file() { return Some(cand); }
    }
    None
}

pub(crate) fn capture(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd).args(args).output().ok()
        .filter(|o| o.status.success() || !o.stdout.is_empty())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}

fn is_running(process_name: &str) -> bool {
    capture("pgrep", &["-x", process_name]).map(|s| !s.trim().is_empty()).unwrap_or(false)
}

pub(crate) fn flatpak_installed(app_id: &str) -> bool {
    capture("flatpak", &["list", "--app", "--columns=application"]).map(|s| s.lines().any(|l| l.trim() == app_id)).unwrap_or(false)
}

// ------------------------------------------------------- which Steam?

/// Which Steam does this PC actually use? v0.7.1's installer blindly
/// installed the native pacman Steam even when the user already had
/// Flatpak Steam — then launches went through the fresh, signed-out
/// native copy and Steam asked to sign in again. Detect the flavour
/// that owns the game library instead, and never install a duplicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SteamFlavour { Flatpak, Native, None }

fn flatpak_steam_root() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_default())
        .join(".var/app/com.valvesoftware.Steam/.local/share/Steam")
}

fn native_steam_roots() -> Vec<PathBuf> {
    let home = PathBuf::from(env::var("HOME").unwrap_or_default());
    vec![home.join(".local/share/Steam"), home.join(".steam/steam")]
}

/// How many installed Steam games live under this root (appmanifests)?
fn steam_game_count(root: &Path) -> usize {
    let Ok(entries) = fs::read_dir(root.join("steamapps")) else { return 0 };
    entries.flatten().filter(|e| {
        let n = e.file_name().to_string_lossy().to_string();
        n.starts_with("appmanifest_") && n.ends_with(".acf")
    }).count()
}

/// Pure flavour choice, given what is installed and where the games are.
/// The flavour with games wins; with games on both, the one with more
/// wins; with no games anywhere, the installed one wins (native on a tie).
pub(crate) fn pick_steam_flavour(flatpak: bool, native: bool, flatpak_games: usize, native_games: usize) -> SteamFlavour {
    if flatpak_games > 0 || native_games > 0 {
        if flatpak_games > native_games { return SteamFlavour::Flatpak; }
        if native_games > flatpak_games { return SteamFlavour::Native; }
        // Equal non-zero counts: prefer the Flatpak library if the native
        // one is a fresh empty-looking install is impossible to tell here,
        // so fall through to the installed-flavour rule below.
    }
    if flatpak && !native { return SteamFlavour::Flatpak; }
    if native && !flatpak { return SteamFlavour::Native; }
    if flatpak && native { return SteamFlavour::Native; }
    SteamFlavour::None
}

pub(crate) fn detect_steam_flavour(_cfg: &Config) -> SteamFlavour {
    let flatpak = flatpak_installed("com.valvesoftware.Steam");
    let native = which("steam").is_some();
    let flatpak_games = steam_game_count(&flatpak_steam_root());
    let native_games = native_steam_roots().iter().map(|r| steam_game_count(r)).sum();
    pick_steam_flavour(flatpak, native, flatpak_games, native_games)
}

impl SteamFlavour {
    pub(crate) fn label(self) -> &'static str {
        match self { SteamFlavour::Flatpak => "Flatpak Steam", SteamFlavour::Native => "native Steam", SteamFlavour::None => "no Steam" }
    }
}

/// The command that launches a Steam game through the Steam the user
/// actually uses. Flatpak Steam is sandboxed: it is launched with
/// `flatpak run … -applaunch`, never by calling a `steam` binary that
/// may be the wrong (duplicate) install.
pub(crate) fn steam_launch_command(flavour: SteamFlavour, appid: u64) -> Command {
    match flavour {
        SteamFlavour::Flatpak => {
            let mut c = Command::new("flatpak");
            c.args(["run", "com.valvesoftware.Steam", "-applaunch", &appid.to_string()]);
            c
        }
        _ => {
            let mut c = Command::new("steam");
            c.args(["-applaunch", &appid.to_string()]);
            c
        }
    }
}

/// Let Flatpak Steam see the WiVRn runtime: grant the sandbox read
/// access to the WiVRn socket + OpenXR/OpenVR config, and symlink the
/// runtime registration into the sandbox config. Without this, games
/// launched from Flatpak Steam open flat on the monitor instead of in
/// the headset. Idempotent — safe to run again.
pub(crate) fn fix_flatpak_steam_visibility() -> Result<String, String> {
    if !flatpak_installed("com.valvesoftware.Steam") {
        return Err("Flatpak Steam is not installed, nothing to fix.".into());
    }
    let mut notes = vec![];
    let out = Command::new("flatpak").args(["override", "--user",
        "--filesystem=xdg-run/wivrn:ro", "--filesystem=xdg-config/openxr:ro",
        "--filesystem=xdg-config/openvr:ro", "com.valvesoftware.Steam"]).output()
        .map_err(|e| format!("flatpak override failed: {e}"))?;
    if out.status.success() { notes.push("flatpak override: granted xdg-run/wivrn, xdg-config/openxr, xdg-config/openvr".to_string()); }
    else { return Err(format!("flatpak override failed: {}", String::from_utf8_lossy(&out.stderr).trim())); }
    let home = PathBuf::from(env::var("HOME").unwrap_or_default());
    let target_dir = home.join(".var/app/com.valvesoftware.Steam/.config/openxr");
    let _ = fs::create_dir_all(&target_dir);
    let link = target_dir.join("1");
    let src = home.join(".config/openxr/1");
    if src.exists() && !link.exists() {
        #[cfg(unix)] { use std::os::unix::fs::symlink; let _ = symlink(&src, &link); }
        notes.push(format!("linked {} → {}", link.display(), src.display()));
    }
    Ok(notes.join("\n"))
}

/// The install plan for this PC: the canonical plan for the distro,
/// minus the Steam package when a Steam is already installed — VRDoctor
/// must never install a duplicate Steam over the one with your games.
pub(crate) fn install_plan(family: &Family) -> Vec<String> {
    let plan = setup_plan_for_family(family);
    if detect_steam_flavour(&load_config()) == SteamFlavour::None { return plan; }
    let mut out = vec![];
    for line in plan {
        let t = line.trim();
        if t.starts_with("sudo pacman -S") && t.contains(" steam") {
            out.push("# Steam already installed — skipped (VRDoctor uses the Steam with your games, no duplicate install)".into());
            out.push(line.replacen(" steam", "", 1));
        } else if t.starts_with("sudo dnf install steam") {
            out.push("# Steam already installed — skipped (VRDoctor uses the Steam with your games, no duplicate install)".into());
            out.push(line.replacen(" steam", "", 1));
        } else { out.push(line); }
    }
    out
}

/// Shell-like argument splitting: honours single/double quotes and backslash escapes.
pub(crate) fn split_args(input: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut has_token = false;
    for ch in input.chars() {
        if escaped { cur.push(ch); escaped = false; continue; }
        match ch {
            '\\' if quote != Some('\'') => escaped = true,
            '\'' | '"' => {
                if quote == Some(ch) { quote = None; } else if quote.is_none() { quote = Some(ch); has_token = true; } else { cur.push(ch); }
            }
            c if c.is_whitespace() && quote.is_none() => {
                if has_token || !cur.is_empty() { out.push(std::mem::take(&mut cur)); has_token = false; }
            }
            c => { cur.push(c); has_token = true; }
        }
    }
    if has_token || !cur.is_empty() { out.push(cur); }
    out
}

// ---------------------------------------------------------------- config

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub backend: String,
    pub steam_root: Option<PathBuf>,
    pub default_refresh: u32,
    /// "dark" (default) or "light"
    pub theme: String,
    /// Accent colour: "teal" (default) or "red"
    pub accent: String,
    /// VR Home environment theme (see HOME_THEMES)
    pub home_theme: String,
    /// VR Home panorama brightness multiplier
    pub home_brightness: f32,
    /// Flip the VR panel texture (an unflipped panel shows upside down in the headset)
    pub flip_panel: bool,
    /// Flip the VR environment (panorama) vertically
    pub flip_env: bool,
    /// VRDoctor VR render resolution scale (0.5-2.0)
    pub render_scale: f32,
    /// VR panel size multiplier (0.7-1.4) — owned by the VR app's
    /// Settings page; the desktop struct only carries it so a desktop
    /// save_config round-trip doesn't silently drop it.
    pub panel_size: f32,
    pub proton: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self { Self { backend: "wivrn".into(), steam_root: None, default_refresh: 90, theme: "dark".into(), accent: "teal".into(), home_theme: "dawn".into(), home_brightness: 1.10, flip_panel: true, flip_env: false, render_scale: 1.0, panel_size: 1.0, proton: None } }
}


// ---------------------------------------------------------------- uninstall

/// The locations VRDoctor keeps its own files in: (config dir, legacy
/// config dir, XDG data home, XDG cache home). Empty XDG variables are
/// treated as unset, per the XDG spec.
fn uninstall_roots() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let home = PathBuf::from(env::var("HOME").unwrap_or_default());
    let xdg = |key: &str, fallback: PathBuf| -> PathBuf {
        match env::var(key) { Ok(v) if !v.trim().is_empty() => PathBuf::from(v), _ => fallback }
    };
    (config_dir(), legacy_config_dir(), xdg("XDG_DATA_HOME", home.join(".local/share")), xdg("XDG_CACHE_HOME", home.join(".cache")))
}

/// Remove everything VRDoctor created in the user's own folders: its
/// config/settings (incl. custom games, VR Home panoramas and the
/// legacy quest3-launcher config), the extracted-games library, the
/// cover-art cache, and its desktop entries (including the WiVRn one).
/// System packages (WiVRn, Steam, ...) are shared with other apps and
/// deliberately left alone. Path-parameterised so tests can run it
/// against a scratch tree; returns a human-readable report.
fn uninstall_at(config: &Path, legacy: &Path, data_home: &Path, cache_home: &Path) -> Vec<String> {
    let mut report = Vec::new();
    for f in ["vrdoctor.desktop", "vrdoctor-vr.desktop"] {
        let p = data_home.join("applications").join(f);
        if p.exists() {
            match fs::remove_file(&p) { Ok(()) => report.push(format!("Removed {}", p.display())), Err(e) => report.push(format!("Could not remove {}: {e}", p.display())) }
        }
    }
    for d in [config.to_path_buf(), legacy.to_path_buf(), data_home.join("vrdoctor"), cache_home.join("vrdoctor")] {
        if d.exists() {
            match fs::remove_dir_all(&d) { Ok(()) => report.push(format!("Removed {}", d.display())), Err(e) => report.push(format!("Could not remove {}: {e}", d.display())) }
        }
    }
    if report.is_empty() { report.push("Nothing to remove — VRDoctor has no files in your user folders.".to_string()); }
    report
}

pub(crate) fn uninstall_user_files() -> Vec<String> {
    let (config, legacy, data_home, cache_home) = uninstall_roots();
    uninstall_at(&config, &legacy, &data_home, &cache_home)
}

/// What an uninstall would remove (the CLI's dry run).
fn uninstall_targets() -> Vec<PathBuf> {
    let (config, legacy, data_home, cache_home) = uninstall_roots();
    let mut v = vec![
        data_home.join("applications/vrdoctor.desktop"),
        data_home.join("applications/vrdoctor-vr.desktop"),
        config, legacy, data_home.join("vrdoctor"), cache_home.join("vrdoctor"),
    ];
    v.retain(|p| p.exists());
    v
}


// ---------------------------------------------------------------- updates

/// A newer release found on GitHub.
#[derive(Debug, Clone)]
pub(crate) struct UpdateInfo {
    pub version: String, // e.g. "0.10.0" (no leading v)
    pub url: String,     // browser_download_url of the AppImage asset
}

/// Parse "0.9.1" / "v0.9.1" into a comparable tuple.
fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches('v');
    let mut it = s.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    if it.next().is_some() { return None; }
    Some((a, b, c))
}

/// The string value of the first `"key": "value"` in a JSON blob
/// (GitHub's API shape is stable; the project hand-rolls JSON elsewhere too).
fn json_string(json: &str, key: &str) -> Option<String> {
    // GitHub's API emits compact JSON ("key":"value", no space).
    let pat = format!("\"{key}\":");
    let rest = json[json.find(&pat)? + pat.len()..].trim_start();
    let rest = rest.strip_prefix('\"')?;
    let end = rest.find('\"')?;
    Some(rest[..end].to_string())
}

/// Extract (tag, AppImage URL) from a GitHub "latest release" payload.
fn release_update(json: &str) -> Option<UpdateInfo> {
    let tag = json_string(json, "tag_name")?;
    // Find the AppImage asset first, then its download URL (the URL key
    // appears after the name key inside the same asset object).
    let asset_at = json.find("VRDoctor-x86_64.AppImage")?;
    let url = json_string(&json[asset_at..], "browser_download_url")?;
    Some(UpdateInfo { version: tag.trim_start_matches('v').to_string(), url })
}

/// Ask GitHub for the latest release; Ok(Some) when it's newer than us.
pub(crate) fn check_for_update() -> Result<Option<UpdateInfo>, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "15", "-A", "vrdoctor-updater",
               "https://api.github.com/repos/profinegamer/vrdoctor/releases/latest"])
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !out.status.success() {
        return Err("GitHub could not be reached (offline, or rate-limited for a while)".into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let info = release_update(&text).ok_or("could not read GitHub's release answer")?;
    let newer = match (parse_version(&info.version), parse_version(env!("CARGO_PKG_VERSION"))) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    };
    Ok(if newer { Some(info) } else { None })
}

/// Download the release AppImage and atomically replace the running one.
/// Only meaningful when launched from the AppImage ($APPIMAGE is set).
pub(crate) fn apply_update(info: &UpdateInfo) -> Result<String, String> {
    let appimage = env::var("APPIMAGE")
        .map_err(|_| "not running from an AppImage — grab the new one from GitHub releases instead".to_string())?;
    let target = PathBuf::from(&appimage);
    let dir = target.parent().ok_or("the AppImage path has no parent folder")?;
    let tmp = dir.join("VRDoctor-update.tmp");
    let _ = fs::remove_file(&tmp);
    let status = Command::new("curl")
        .args(["-fL", "--max-time", "900", "-o"])
        .arg(&tmp)
        .arg(&info.url)
        .status()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !status.success() {
        let _ = fs::remove_file(&tmp);
        return Err("the download failed — check your connection and try again".into());
    }
    // Sanity: an AppImage is an ELF at least a megabyte in size.
    let head = fs::read(&tmp).unwrap_or_default();
    if head.len() < 1_000_000 || head.get(..4) != Some(&[0x7f, b'E', b'L', b'F'][..]) {
        let _ = fs::remove_file(&tmp);
        return Err("the download didn't look like a VRDoctor AppImage, so nothing was changed".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755));
    }
    fs::rename(&tmp, &target).map_err(|e| format!("could not replace the old AppImage: {e}"))?;
    Ok(format!("Updated to v{} — restart VRDoctor to use it.", info.version))
}

fn cmd_update(args: &[String]) {
    let check_only = args.iter().any(|a| a == "--check");
    println!("Checking GitHub for a newer VRDoctor…");
    match check_for_update() {
        Ok(Some(info)) => {
            println!("v{} is available (you have v{}).", info.version, env!("CARGO_PKG_VERSION"));
            if check_only {
                println!("Download it from https://github.com/profinegamer/vrdoctor/releases/latest — or run `vrdoctor update` and I'll fetch it.");
                return;
            }
            match apply_update(&info) {
                Ok(m) => println!("{m}"),
                Err(e) => { eprintln!("Update failed: {e}"); std::process::exit(1); }
            }
        }
        Ok(None) => println!("You're up to date (v{}).", env!("CARGO_PKG_VERSION")),
        Err(e) => { eprintln!("{e}"); std::process::exit(1); }
    }
}

fn cmd_uninstall(args: &[String]) {
    if !args.iter().any(|a| a == "--yes") {
        let targets = uninstall_targets();
        if targets.is_empty() { println!("Nothing to remove — VRDoctor has no files in your user folders."); return; }
        println!("This removes VRDoctor's own files (system packages like WiVRn are left alone):");
        for t in targets { println!("  {}", t.display()); }
        println!("\nNothing was removed. Run `vrdoctor uninstall --yes` to go ahead — and delete the VRDoctor AppImage file itself afterwards.");
        return;
    }
    for line in uninstall_user_files() { println!("{line}"); }
    println!("\nDone. The VR packages (WiVRn and friends) were left in place — remove them with your package manager if you want them gone. Delete the VRDoctor AppImage file itself to finish.");
}

pub(crate) fn config_dir() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".config"))
        .join("vrdoctor")
}

fn legacy_config_dir() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".config"))
        .join("quest3-launcher")
}

pub(crate) fn config_path() -> PathBuf { config_dir().join("config.conf") }

pub(crate) fn load_config() -> Config {
    let mut cfg = Config::default();
    // New location first, legacy quest3-launcher config as fallback.
    let text = fs::read_to_string(config_path())
        .or_else(|_| fs::read_to_string(legacy_config_dir().join("config.conf")))
        .unwrap_or_default();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "backend" => cfg.backend = v.trim().to_string(),
                "steam_root" => cfg.steam_root = Some(PathBuf::from(v.trim())),
                "default_refresh" => cfg.default_refresh = v.trim().parse().unwrap_or(90),
                "theme" => cfg.theme = v.trim().to_string(),
                "accent" => cfg.accent = v.trim().to_string(),
                "home_theme" => cfg.home_theme = v.trim().to_string(),
                "home_brightness" => cfg.home_brightness = v.trim().parse().unwrap_or(1.10),
                "flip_panel" => cfg.flip_panel = matches!(v.trim(), "1" | "true"),
                "flip_env" => cfg.flip_env = matches!(v.trim(), "1" | "true"),
                "render_scale" => cfg.render_scale = v.trim().parse::<f32>().unwrap_or(1.0).clamp(0.5, 2.0),
                "panel_size" => cfg.panel_size = v.trim().parse::<f32>().unwrap_or(1.0).clamp(0.7, 1.4),
                "proton" => cfg.proton = Some(PathBuf::from(v.trim())),
                _ => {}
            }
        }
    }
    cfg
}

pub(crate) fn save_config(cfg: &Config) -> std::io::Result<PathBuf> {
    let p = config_path();
    if let Some(parent) = p.parent() { fs::create_dir_all(parent)?; }
    let mut s = format!("# vrdoctor config\nbackend={}\ndefault_refresh={}\ntheme={}\naccent={}\nhome_theme={}\nhome_brightness={:.2}\nflip_panel={}\nflip_env={}\nrender_scale={:.2}\npanel_size={:.2}\n", cfg.backend, cfg.default_refresh, cfg.theme, cfg.accent, cfg.home_theme, cfg.home_brightness, cfg.flip_panel as u8, cfg.flip_env as u8, cfg.render_scale, cfg.panel_size);
    if let Some(r) = &cfg.steam_root { s.push_str(&format!("steam_root={}\n", r.display())); }
    if let Some(pr) = &cfg.proton { s.push_str(&format!("proton={}\n", pr.display())); }
    fs::write(&p, s)?;
    Ok(p)
}

fn save_default_config() -> std::io::Result<PathBuf> {
    if config_path().exists() { return Ok(config_path()); }
    save_config(&Config::default())
}

// ---------------------------------------------------------------- steam

pub(crate) fn steam_roots(cfg: &Config) -> Vec<PathBuf> {
    let home = PathBuf::from(env::var("HOME").unwrap_or_default());
    let mut roots = vec![];
    if let Some(r) = &cfg.steam_root { roots.push(r.clone()); }
    roots.extend([
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ]);
    let mut extra = vec![];
    for root in &roots {
        if let Ok(vdf) = fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
            for line in vdf.lines() {
                let t = line.trim();
                if t.starts_with("\"path\"") {
                    if let Some(path) = t.split('"').nth(3) { extra.push(PathBuf::from(path)); }
                }
            }
        }
    }
    roots.extend(extra);
    roots.sort(); roots.dedup();
    roots.into_iter().filter(|r| r.join("steamapps").is_dir()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Game { pub appid: u64, pub name: String, pub library: PathBuf, pub install_dir: PathBuf }

pub(crate) fn parse_appmanifest(content: &str) -> Option<(u64, String)> {
    let (appid, name, _) = parse_appmanifest_full(content)?;
    Some((appid, name))
}

/// Full manifest parse: (appid, name, installdir-name).
pub(crate) fn parse_appmanifest_full(content: &str) -> Option<(u64, String, String)> {
    let mut appid = None; let mut name = None; let mut installdir = String::new();
    for line in content.lines() {
        let parts: Vec<&str> = line.split('"').collect();
        if parts.len() >= 4 {
            match parts[1] {
                "appid" => appid = parts[3].parse().ok(),
                "name" => name = Some(parts[3].to_string()),
                "installdir" => installdir = parts[3].to_string(),
                _ => {}
            }
        }
    }
    Some((appid?, name?, installdir))
}

pub(crate) fn list_games(cfg: &Config) -> Vec<Game> {
    let mut games = vec![];
    for root in steam_roots(cfg) {
        let apps = root.join("steamapps");
        if let Ok(entries) = fs::read_dir(&apps) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("appmanifest_") && name.ends_with(".acf") {
                    if let Ok(text) = fs::read_to_string(e.path()) {
                        if let Some((appid, gname, installdir)) = parse_appmanifest_full(&text) {
                            let dir = root.join("steamapps/common").join(&installdir);
                            games.push(Game { appid, name: gname, library: root.clone(), install_dir: dir });
                        }
                    }
                }
            }
        }
    }
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    games
}

/// Find a Proton binary: config first, then Steam's compatibilitytools.d / common folders.
pub(crate) fn find_proton(cfg: &Config) -> Option<PathBuf> {
    if let Some(p) = &cfg.proton { if p.is_file() { return Some(p.clone()); } }
    let home = PathBuf::from(env::var("HOME").unwrap_or_default());
    let mut search_roots = steam_roots(cfg);
    search_roots.push(home.join(".local/share/Steam"));
    for root in search_roots {
        for base in [root.join("compatibilitytools.d"), root.join("steamapps/common")] {
            if let Ok(entries) = fs::read_dir(&base) {
                let mut candidates: Vec<PathBuf> = entries.flatten()
                    .map(|e| e.path().join("proton"))
                    .filter(|p| p.is_file())
                    .collect();
                candidates.sort();
                if let Some(p) = candidates.into_iter().last() { return Some(p); }
            }
        }
    }
    None
}

// ------------------------------------------------------- Steam VR / 2D detection

/// Is a Steam game a VR game, a hybrid (VR + 2D), a flat 2D game, or unknown?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VrSupport { VrOnly, VrSupported, Flat, Unknown }

impl VrSupport {
    pub(crate) fn label(&self) -> &'static str {
        match self { VrSupport::VrOnly => "VR", VrSupport::VrSupported => "VR+2D", VrSupport::Flat => "2D", VrSupport::Unknown => "?" }
    }
    fn to_cache(&self) -> Option<&'static str> {
        match self { VrSupport::VrOnly => Some("vr"), VrSupport::VrSupported => Some("hybrid"), VrSupport::Flat => Some("flat"), VrSupport::Unknown => None }
    }
    fn from_cache(s: &str) -> VrSupport {
        match s { "vr" => VrSupport::VrOnly, "hybrid" => VrSupport::VrSupported, "flat" => VrSupport::Flat, _ => VrSupport::Unknown }
    }
}

/// Well-known VR-only Steam titles (AppIDs), incl. the free ones used by
/// scripts/test-free-games.sh.
const KNOWN_VR_ONLY: &[u64] = &[
    546560, // Half-Life: Alyx
    620980, // Beat Saber
    450390, // The Lab (free)
    348250, // Google Earth VR (free)
    518580, // Accounting (free)
    1363430, // Propagation VR (free)
    381940, // Trials on Tatooine (free)
    868020, // Aperture Hand Lab (free)
    468700, // NVIDIA VR Funhouse (free)
    457550, // Bigscreen Beta (free)
    1069890, // Moondust: Knuckles Tech Demos (free)
    823500, // BONEWORKS
    629730, // Blade & Sorcery
    1012790, // Into the Radius VR
    617830, // SUPERHOT VR
    650000, // DOOM VFR
    611660, // Fallout 4 VR
    611670, // The Elder Scrolls V: Skyrim VR
    1033830, // Star Wars: Droid Repair Bay (free)
    665720, // Waltz of the Wizard
];

/// Well-known hybrid titles: playable flat AND in VR.
const KNOWN_VR_SUPPORTED: &[u64] = &[
    438100, // VRChat (free)
    471710, // Rec Room (free)
    223750, // DCS World (free base)
    236390, // War Thunder (free)
    275850, // No Man's Sky
    739630, // Phasmophobia
    359320, // Elite Dangerous
    1250410, // Microsoft Flight Simulator (2020)
    690790, // DiRT Rally 2.0
    805550, // Assetto Corsa Competizione
    578650, // The Forest
    264710, // Subnautica
    555160, // Pavlov VR (VR-first, listed as supported in some regions)
];

fn vr_cache_path() -> PathBuf { config_dir().join("steam_vr_cache.conf") }

pub(crate) fn load_vr_cache() -> BTreeMap<u64, VrSupport> {
    let mut map = BTreeMap::new();
    if let Ok(text) = fs::read_to_string(vr_cache_path()) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                if let Ok(id) = k.trim().parse::<u64>() { map.insert(id, VrSupport::from_cache(v.trim())); }
            }
        }
    }
    map
}

fn save_vr_cache(map: &BTreeMap<u64, VrSupport>) -> std::io::Result<()> {
    let p = vr_cache_path();
    if let Some(parent) = p.parent() { fs::create_dir_all(parent)?; }
    let mut s = String::from("# VRDoctor Steam VR-support cache (from the Steam Store API)\n");
    for (id, sup) in map { if let Some(v) = sup.to_cache() { s.push_str(&format!("{id}={v}\n")); } }
    fs::write(p, s)
}

/// Does the game's install folder contain OpenVR/OpenXR runtime files?
/// (Unity/Unreal VR games ship openvr_api / openxr loader plugins.)
pub(crate) fn install_has_vr_markers(dir: &Path) -> bool {
    fn walk(dir: &Path, depth: u32) -> bool {
        if depth > 3 || !dir.is_dir() { return false; }
        let Ok(entries) = fs::read_dir(dir) else { return false };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.contains("openvr") || name.contains("openxr") || name == "steamvr" || name.contains("unityopenxr") {
                return true;
            }
            if p.is_dir() && walk(&p, depth + 1) { return true; }
        }
        false
    }
    walk(dir, 0)
}

/// Offline classification: cache → known titles → install-folder markers.
pub(crate) fn classify_offline(game: &Game) -> (VrSupport, &'static str) {
    if let Some(s) = load_vr_cache().get(&game.appid) { return (*s, "Steam (cached)"); }
    if KNOWN_VR_ONLY.contains(&game.appid) { return (VrSupport::VrOnly, "known VR title"); }
    if KNOWN_VR_SUPPORTED.contains(&game.appid) { return (VrSupport::VrSupported, "known VR+2D title"); }
    if install_has_vr_markers(&game.install_dir) { return (VrSupport::VrSupported, "VR files in install folder"); }
    (VrSupport::Unknown, "not checked online yet")
}

/// Parse a Steam Store appdetails response into a VR verdict.
/// Matches the store category descriptions exactly ("VR Only" / "VR Supported").
pub(crate) fn parse_store_categories(json: &str) -> Option<VrSupport> {
    if json.contains("\"success\":false") { return None; }
    if json.contains("\"description\":\"VR Only\"") { return Some(VrSupport::VrOnly); }
    if json.contains("\"description\":\"VR Supported\"") || json.contains("\"description\":\"Tracked Controller Support\"") {
        return Some(VrSupport::VrSupported);
    }
    if json.contains("\"success\":true") { return Some(VrSupport::Flat); }
    None
}

/// Ask the Steam Store API (via curl, no API key needed) and cache the answer.
pub(crate) fn online_classify(appid: u64) -> Option<VrSupport> {
    let url = format!("https://store.steampowered.com/api/appdetails?appids={appid}");
    let json = capture("curl", &["-s", "--max-time", "12", &url])?;
    let verdict = parse_store_categories(&json)?;
    let mut cache = load_vr_cache();
    cache.insert(appid, verdict);
    let _ = save_vr_cache(&cache);
    Some(verdict)
}

/// Full classification. With `online`, unknown games are checked against Steam.
pub(crate) fn classify_game(game: &Game, online: bool) -> (VrSupport, String) {
    let (sup, src) = classify_offline(game);
    if sup != VrSupport::Unknown || !online { return (sup, src.to_string()); }
    match online_classify(game.appid) {
        Some(s) => (s, "Steam Store API".to_string()),
        None => (VrSupport::Unknown, "Steam check failed (offline?)".to_string()),
    }
}

pub(crate) fn classify_appid(appid: u64, online: bool) -> (VrSupport, String) {
    let game = Game { appid, name: String::new(), library: PathBuf::new(), install_dir: PathBuf::new() };
    // Prefer the installed game's real record when we have it (for markers).
    let cfg = load_config();
    if let Some(g) = list_games(&cfg).into_iter().find(|g| g.appid == appid) { return classify_game(&g, online); }
    classify_game(&game, online)
}

/// Launch a Steam game, auto-routing flat games to the big screen (2D mode)
/// and VR/hybrid games to the headset.
pub(crate) fn launch_steam_auto(appid: u64, backend: &str, cfg: &Config) -> Result<String, String> {
    let (sup, src) = classify_appid(appid, false);
    match sup {
        VrSupport::Flat => {
            let m = launch_steam_2d(appid, backend)?;
            Ok(format!("Auto-detected as a 2D game ({src}) — routed to the big screen.\n{m}"))
        }
        _ => {
            let note = match sup {
                VrSupport::VrOnly => format!("Auto-detected as a VR game ({src})."),
                VrSupport::VrSupported => format!("Auto-detected as VR+2D ({src}) — launching in VR. Use play2d for the big screen."),
                _ => format!("VR support unknown ({src}) — launching in VR. Tip: `vrdoctor list-games --online` checks Steam."),
            };
            let m = launch_steam_appid(appid, backend)?;
            Ok(format!("{note}\n{m}"))
        }
    }
}

// ------------------------------------------------------- Steam cover art

/// Where downloaded Steam capsule art is cached (~/.cache/vrdoctor/art).
pub(crate) fn art_dir() -> PathBuf {
    let base = env::var("XDG_CACHE_HOME").map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".cache"));
    base.join("vrdoctor/art")
}

pub(crate) fn art_path(appid: u64) -> PathBuf { art_dir().join(format!("{appid}.jpg")) }

/// Steam CDN artwork for a game, best first: portrait library capsule,
/// then the wide capsule, then the header.
pub(crate) fn art_candidates(appid: u64) -> Vec<String> {
    ["library_600x900.jpg", "capsule_616x353.jpg", "header.jpg"].iter()
        .map(|f| format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/{f}"))
        .collect()
}

/// Download a game's Steam capsule art into the cache (via curl).
/// Writes to a .part file and renames, so readers never see a partial file.
pub(crate) fn download_art(appid: u64) -> bool {
    let dir = art_dir();
    if fs::create_dir_all(&dir).is_err() { return false; }
    let tmp = dir.join(format!("{appid}.part"));
    for url in art_candidates(appid) {
        let ok = std::process::Command::new("curl")
            .args(["-fsSL", "--max-time", "10", "-o"])
            .arg(&tmp).arg(&url)
            .status().map(|s| s.success()).unwrap_or(false);
        if ok && fs::metadata(&tmp).map(|m| m.len() > 3000).unwrap_or(false)
            && fs::rename(&tmp, art_path(appid)).is_ok() { return true; }
    }
    let _ = fs::remove_file(&tmp);
    false
}

// ------------------------------------------------- machine-readable game list

/// Escape a string for inclusion in a JSON string literal.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// The game list as a JSON array — the contract `vrdoctor-vr` (the
/// in-headset UI) consumes via `vrdoctor list-games --json`.
/// Steam entries: {"kind":"steam","appid":N,"name":"…","vr":"vr|vr2d|2d|unknown"}
/// Custom entries: {"kind":"custom","appid":0,"name":"…","vr":"vr|2d"}
/// Classification is offline (cache + known titles + install markers).
pub(crate) fn games_json(games: &[Game], custom: &[CustomGame]) -> String {
    let vr_str = |sup: VrSupport| match sup {
        VrSupport::VrOnly => "vr",
        VrSupport::VrSupported => "vr2d",
        VrSupport::Flat => "2d",
        VrSupport::Unknown => "unknown",
    };
    let mut out = String::from("[");
    let mut first = true;
    for g in games {
        if !first { out.push(','); }
        first = false;
        out.push_str(&format!(
            "{{\"kind\":\"steam\",\"appid\":{},\"name\":\"{}\",\"vr\":\"{}\"}}",
            g.appid, json_escape(&g.name), vr_str(classify_game(g, false).0)
        ));
    }
    for g in custom {
        if !first { out.push(','); }
        first = false;
        out.push_str(&format!(
            "{{\"kind\":\"custom\",\"appid\":0,\"name\":\"{}\",\"vr\":\"{}\"}}",
            json_escape(&g.name), if g.mode == "2d" { "2d" } else { "vr" }
        ));
    }
    out.push(']');
    out
}

// ---------------------------------------------------------------- custom games

/// A game that is not on Steam or any other store. `path` can point at
/// almost any reasonable game file — see `GameKind` for the full list.
/// Archives and folders are resolved to the real executable inside them
/// when the game is added; the original location is kept in `source`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct CustomGame {
    pub name: String,
    pub path: PathBuf,
    /// Extra arguments, as typed (parsed with `split_args` at launch time).
    pub args: String,
    pub workdir: Option<PathBuf>,
    /// Backend override; empty = use the global default.
    pub backend: String,
    /// Proton binary override for .exe games; empty = auto-detect.
    pub proton: String,
    /// Original archive / folder the executable was resolved from, if any.
    pub source: String,
    /// "vr" (default) or "2d" — 2D games play on the big virtual screen in VR.
    pub mode: String,
}

pub(crate) fn custom_games_path() -> PathBuf { config_dir().join("custom_games.conf") }

pub(crate) fn parse_custom_games(content: &str) -> Vec<CustomGame> {
    let mut games = vec![];
    let mut cur = CustomGame::default();
    let mut in_block = false;
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            if in_block && !cur.name.is_empty() && !cur.path.as_os_str().is_empty() {
                games.push(std::mem::take(&mut cur));
            } else { cur = CustomGame::default(); }
            in_block = false;
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            in_block = true;
            match k.trim() {
                "name" => cur.name = v.trim().to_string(),
                "path" => cur.path = PathBuf::from(v.trim()),
                "args" => cur.args = v.trim().to_string(),
                "workdir" => cur.workdir = Some(PathBuf::from(v.trim())),
                "backend" => cur.backend = v.trim().to_string(),
                "proton" => cur.proton = v.trim().to_string(),
                "source" => cur.source = v.trim().to_string(),
                "mode" => cur.mode = v.trim().to_string(),
                _ => {}
            }
        }
    }
    if in_block && !cur.name.is_empty() && !cur.path.as_os_str().is_empty() {
        games.push(cur);
    }
    games
}

pub(crate) fn load_custom_games() -> Vec<CustomGame> {
    fs::read_to_string(custom_games_path())
        .or_else(|_| fs::read_to_string(legacy_config_dir().join("custom_games.conf")))
        .map(|t| parse_custom_games(&t))
        .unwrap_or_default()
}

pub(crate) fn save_custom_games(games: &[CustomGame]) -> std::io::Result<PathBuf> {
    let p = custom_games_path();
    if let Some(parent) = p.parent() { fs::create_dir_all(parent)?; }
    let mut s = String::from("# VRDoctor custom (non-store) games. One block per game, blank line between games.\n# Edit by hand, via `vrdoctor add-game`, or in the GUI.\n\n");
    for g in games {
        s.push_str(&format!("name={}\npath={}\n", g.name, g.path.display()));
        if !g.args.is_empty() { s.push_str(&format!("args={}\n", g.args)); }
        if let Some(w) = &g.workdir { s.push_str(&format!("workdir={}\n", w.display())); }
        if !g.backend.is_empty() { s.push_str(&format!("backend={}\n", g.backend)); }
        if !g.proton.is_empty() { s.push_str(&format!("proton={}\n", g.proton)); }
        if !g.source.is_empty() { s.push_str(&format!("source={}\n", g.source)); }
        if g.mode == "2d" { s.push_str("mode=2d\n"); }
        s.push('\n');
    }
    fs::write(&p, s)?;
    Ok(p)
}

pub(crate) fn add_custom_game(game: CustomGame) -> std::io::Result<PathBuf> {
    let mut games = load_custom_games();
    games.retain(|g| g.name.to_lowercase() != game.name.to_lowercase());
    games.push(game);
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    save_custom_games(&games)
}

pub(crate) fn remove_custom_game(name: &str) -> std::io::Result<bool> {
    let mut games = load_custom_games();
    let before = games.len();
    games.retain(|g| g.name.to_lowercase() != name.to_lowercase());
    let removed = games.len() != before;
    if removed { save_custom_games(&games)?; }
    Ok(removed)
}

#[allow(dead_code)] // kept as a small helper for callers/tests that only need a yes/no
fn is_windows_binary(path: &Path) -> bool {
    matches!(detect_kind(path), GameKind::WindowsExe | GameKind::WindowsBatch | GameKind::WindowsInstaller)
}

// ------------------------------------------------------- file kinds

/// Every kind of game file VRDoctor accepts ("within reason"):
/// native executables and AppImages, shell/Python/Perl/Ruby/Node scripts,
/// Java .jar, LÖVE .love, Windows .exe/.bat/.cmd/.msi, .desktop entries,
/// Quest .apk (sideload), Flatpak bundles/refs, game archives, and folders
/// containing any of the above. Anything else executable (ELF magic,
/// shebang, +x bit) is treated as a native executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GameKind {
    NativeExecutable, AppImage, ShellScript, Python, Perl, Ruby, Node,
    Jar, Love, WindowsExe, WindowsBatch, WindowsInstaller,
    DesktopEntry, AndroidApk, FlatpakBundle, FlatpakRef,
    Archive, Directory, Unknown,
}

pub(crate) fn detect_kind(path: &Path) -> GameKind {
    if path.is_dir() { return GameKind::Directory; }
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    // Multi-part archive extensions first.
    for suf in [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".tgz", ".tbz2", ".txz"] {
        if name.ends_with(suf) { return GameKind::Archive; }
    }
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
        Some("appimage") => GameKind::AppImage,
        Some("sh") | Some("bash") | Some("zsh") | Some("run") => GameKind::ShellScript,
        Some("py") | Some("pyw") => GameKind::Python,
        Some("pl") => GameKind::Perl,
        Some("rb") => GameKind::Ruby,
        Some("js") | Some("mjs") => GameKind::Node,
        Some("jar") => GameKind::Jar,
        Some("love") => GameKind::Love,
        Some("exe") | Some("com") | Some("scr") => GameKind::WindowsExe,
        Some("bat") | Some("cmd") => GameKind::WindowsBatch,
        Some("msi") => GameKind::WindowsInstaller,
        Some("desktop") => GameKind::DesktopEntry,
        Some("apk") => GameKind::AndroidApk,
        Some("flatpak") => GameKind::FlatpakBundle,
        Some("flatpakref") => GameKind::FlatpakRef,
        Some("zip") | Some("7z") | Some("rar") | Some("tar") | Some("gz") | Some("xz") | Some("bz2") | Some("zst") => GameKind::Archive,
        _ => {
            // No / unrecognised extension: accept anything that looks runnable.
            if has_elf_magic(path) || has_shebang(path) || is_executable_file(path) { GameKind::NativeExecutable } else { GameKind::Unknown }
        }
    }
}

fn read_head(path: &Path, n: usize) -> Vec<u8> {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    if let Ok(mut f) = fs::File::open(path) { let _ = f.read(&mut buf); }
    buf
}

fn has_elf_magic(path: &Path) -> bool { read_head(path, 4) == [0x7f, b'E', b'L', b'F'] }
fn has_shebang(path: &Path) -> bool { read_head(path, 2) == [b'#', b'!'] }

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}
#[cfg(not(unix))]
fn is_executable_file(_path: &Path) -> bool { false }

#[cfg(unix)]
fn ensure_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        if meta.permissions().mode() & 0o111 == 0 {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o755);
            let _ = fs::set_permissions(path, perms);
        }
    }
}
#[cfg(not(unix))]
fn ensure_executable(_path: &Path) {}

/// Rank executables found inside a folder/archive: prefer native Linux
/// builds, then scripts/jars, then Windows builds (they need Proton/Wine).
fn executable_rank(path: &Path) -> u32 {
    match detect_kind(path) {
        GameKind::AppImage => 0,
        GameKind::NativeExecutable => 1,
        GameKind::ShellScript => 2,
        GameKind::Jar => 3,
        GameKind::Python | GameKind::Perl | GameKind::Ruby | GameKind::Node | GameKind::Love => 4,
        GameKind::WindowsExe => 5,
        GameKind::WindowsBatch => 6,
        _ => 99,
    }
}

pub(crate) fn find_executable_in(dir: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, depth: u32, best: &mut Option<(u32, PathBuf)>) {
        if depth > 3 { return; }
        let Ok(entries) = fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                // Skip obvious non-game subtrees.
                let n = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                if ["__macosx", ".git", "docs", "documentation"].contains(&n.as_str()) { continue; }
                walk(&p, depth + 1, best);
            } else {
                let rank = executable_rank(&p);
                if rank < 99 {
                    // Prefer shallower, higher-ranked files; a file named like the folder wins ties.
                    let score = rank * 10 + depth;
                    if best.as_ref().map(|(s, _)| score < *s).unwrap_or(true) { *best = Some((score, p)); }
                }
            }
        }
    }
    let mut best = None;
    walk(dir, 0, &mut best);
    best.map(|(_, p)| p)
}

pub(crate) fn library_dir() -> PathBuf {
    env::var_os("XDG_DATA_HOME").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".local/share"))
        .join("vrdoctor/library")
}

fn sanitize_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if s.is_empty() { "game".into() } else { s }
}

/// Extract an archive into the VRDoctor library and return the game executable inside.
pub(crate) fn extract_archive(archive: &Path, game_name: &str) -> Result<PathBuf, String> {
    let dest = library_dir().join(sanitize_name(game_name));
    if let Some(exe) = find_executable_in(&dest) { return Ok(exe); } // already extracted
    fs::create_dir_all(&dest).map_err(|e| format!("Could not create {}: {e}", dest.display()))?;
    let name = archive.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    let (prog, args): (&str, Vec<String>) = if name.ends_with(".zip") && which("unzip").is_some() {
        ("unzip", vec!["-o".into(), "-q".into(), archive.display().to_string(), "-d".into(), dest.display().to_string()])
    } else if which("bsdtar").is_some() {
        ("bsdtar", vec!["-xf".into(), archive.display().to_string(), "-C".into(), dest.display().to_string()])
    } else if name.ends_with(".7z") || name.ends_with(".rar") {
        if which("7z").is_some() || which("7zz").is_some() {
            ("7z", vec!["x".into(), "-y".into(), format!("-o{}", dest.display()), archive.display().to_string()])
        } else {
            return Err("Extracting .7z/.rar needs a tool: install p7zip (Arch: p7zip, Fedora: p7zip) or bsdtar (libarchive-tools / libarchive).".into());
        }
    } else {
        // tar handles .tar, .tar.gz/.tgz, .tar.xz, .tar.bz2, .tar.zst (with zstd installed).
        ("tar", vec!["-xf".into(), archive.display().to_string(), "-C".into(), dest.display().to_string()])
    };
    let status = Command::new(prog).args(&args).status().map_err(|e| format!("Could not run {prog} to extract {}: {e}", archive.display()))?;
    if !status.success() { return Err(format!("{prog} failed to extract {}", archive.display())); }
    find_executable_in(&dest).ok_or_else(|| format!("Extracted {} but found no runnable game file inside (looked for executables, .AppImage, .sh, .jar, .exe …).", archive.display()))
}

/// Resolve folders and archives to the real executable, in place.
/// Returns the (possibly rewritten) game plus a human-readable note.
pub(crate) fn resolve_game_target(game: &CustomGame) -> Result<(CustomGame, String), String> {
    match detect_kind(&game.path) {
        GameKind::Directory => {
            let exe = find_executable_in(&game.path)
                .ok_or_else(|| format!("No runnable game file found in folder {} (looked up to 3 levels deep for executables, .AppImage, .sh, .jar, .exe …).", game.path.display()))?;
            let mut g = game.clone();
            let note = format!("Folder resolved: {} → {}", game.path.display(), exe.display());
            g.source = game.path.display().to_string();
            g.path = exe;
            Ok((g, note))
        }
        GameKind::Archive => {
            let exe = extract_archive(&game.path, &game.name)?;
            let mut g = game.clone();
            let note = format!("Archive extracted to the VRDoctor library; executable: {}", exe.display());
            g.source = game.path.display().to_string();
            g.path = exe;
            Ok((g, note))
        }
        GameKind::Unknown => Err(format!("VRDoctor doesn't know how to run {} — supported: executables (ELF/shebang/+x), .AppImage, .sh, .py, .pl, .rb, .js, .jar, .love, .exe/.bat/.cmd/.msi, .desktop, .apk, .flatpak/.flatpakref, archives (.zip, .tar.gz, .tar.xz, .7z, .rar …), and folders containing a game.", game.path.display())),
        _ => Ok((game.clone(), String::new())),
    }
}

/// Add a custom game, resolving folders/archives first. Used by CLI and GUI.
pub(crate) fn register_custom_game(game: CustomGame) -> Result<(CustomGame, String, PathBuf), String> {
    let (resolved, note) = resolve_game_target(&game)?;
    let saved = add_custom_game(resolved.clone()).map_err(|e| format!("Failed to save: {e}"))?;
    Ok((resolved, note, saved))
}

/// Parse the Exec= line of a .desktop file into (program, args), dropping
/// freedesktop field codes (%f, %u, %U, …).
pub(crate) fn parse_desktop_exec(content: &str) -> Option<(String, Vec<String>)> {
    for line in content.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Exec=") {
            let tokens: Vec<String> = split_args(rest).into_iter().filter(|a| !(a.starts_with('%') && a.len() <= 2)).collect();
            if let Some((prog, args)) = tokens.split_first() { return Some((prog.clone(), args.to_vec())); }
        }
    }
    None
}

pub(crate) const SUPPORTED_KINDS: &str = "executables (ELF / shebang / +x, any or no extension), .AppImage, scripts (.sh .bash .py .pl .rb .js), .jar, .love, Windows (.exe .com .bat .cmd .msi), .desktop, Quest .apk, .flatpak / .flatpakref, archives (.zip .tar .tar.gz .tar.xz .tar.bz2 .tar.zst .7z .rar), and folders containing a game";

// ---------------------------------------------------------------- quest

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdbDevice { pub serial: String, pub state: String }

pub(crate) fn parse_adb_devices(output: &str) -> Vec<AdbDevice> {
    output.lines().skip(1).filter_map(|l| {
        let mut it = l.split_whitespace();
        Some(AdbDevice { serial: it.next()?.to_string(), state: it.next()?.to_string() })
    }).collect()
}

pub(crate) fn detect_quest() -> Vec<(AdbDevice, Option<String>, Option<String>)> {
    let Some(out) = capture("adb", &["devices"]) else { return vec![] };
    parse_adb_devices(&out).into_iter().map(|d| {
        let model = capture("adb", &["-s", &d.serial, "shell", "getprop", "ro.product.model"]).map(|s| s.trim().to_string());
        let device = capture("adb", &["-s", &d.serial, "shell", "getprop", "ro.product.device"]).map(|s| s.trim().to_string());
        (d, model, device)
    }).collect()
}

pub(crate) fn is_quest3(model: &Option<String>, device: &Option<String>) -> bool {
    let m = format!("{} {}", model.clone().unwrap_or_default(), device.clone().unwrap_or_default()).to_lowercase();
    m.contains("quest 3") || m.contains("eureka")
}

// ---------------------------------------------------------------- doctor / checks

#[derive(Debug, Clone)]
pub(crate) struct CheckResult { pub label: String, pub ok: bool, pub detail: String }

pub(crate) fn run_checks() -> Vec<CheckResult> {
    let distro = current_distro();
    let cfg = load_config();
    let mut out = vec![];
    let mut push = |label: &str, ok: bool, detail: String| out.push(CheckResult { label: label.to_string(), ok, detail });

    push("Distro", matches!(distro.family, Family::Arch | Family::Fedora),
        format!("{} (id={}, id_like={}) — Arch/Fedora get scripted setup; other distros can use the WiVRn Flatpak", distro.pretty, distro.id, distro.id_like));

    let gpu = capture("lspci", &[]).unwrap_or_default();
    let gpu_line = gpu.lines().find(|l| l.contains("VGA") || l.contains("3D controller")).unwrap_or("unknown GPU").trim().to_string();
    let has_gpu = ["NVIDIA", "AMD", "ATI", "Intel"].iter().any(|v| gpu_line.contains(v));
    push("GPU", has_gpu, gpu_line);

    let flavour = detect_steam_flavour(&cfg);
    push("Steam", flavour != SteamFlavour::None,
        match flavour {
            SteamFlavour::Flatpak => "Flatpak Steam — your games live here, VRDoctor launches through `flatpak run com.valvesoftware.Steam` and will not install a duplicate native Steam".into(),
            SteamFlavour::Native => "native Steam — VRDoctor launches through the `steam` binary".into(),
            SteamFlavour::None => "not found — run: vrdoctor setup".into(),
        });
    push("adb (Android tools)", which("adb").is_some(), "needed for USB detection / sideloading the WiVRn or ALVR client".into());
    push("Avahi (mDNS)", is_running("avahi-daemon") || which("avahi-daemon").is_some(), "WiVRn discovery needs avahi-daemon running + UDP 5353 open".into());
    push("PipeWire", is_running("pipewire") || which("pipewire").is_some(), "audio on Linux VR goes through PipeWire".into());

    let wivrn_native = which("wivrn-server").is_some() || which("wivrn-dashboard").is_some();
    let wivrn_flatpak = flatpak_installed("io.github.wivrn.wivrn");
    push("WiVRn (recommended)", wivrn_native || wivrn_flatpak,
        if wivrn_native { "native package found".into() } else if wivrn_flatpak { "Flatpak found".into() } else { "not found — run: vrdoctor setup".into() });

    let alvr = which("alvr_dashboard").is_some() || which("ALVR").is_some() || which("alvr_streamer").is_some();
    push("ALVR (fallback)", alvr, if alvr { "found".into() } else { "not found (optional)".into() });

    let overlay = which("wayvr").is_some() || which("wlx-overlay-s").is_some();
    push("2D overlay (WayVR/wlx)", overlay, if overlay { "found — 2D games + desktop can show in VR".into() } else { "not found (optional, needed for 2D mode) — Arch: yay -S wayvr · Fedora: see WayVR releases".into() });
    let steamvr = steam_roots(&cfg).iter().any(|r| r.join("steamapps/common/SteamVR").is_dir());
    push("SteamVR (NOT required)", true, if steamvr { "installed, but VRDoctor's WiVRn route never uses it — only the optional ALVR/Steam Link backends need it".into() } else { "not installed — fine, nothing in VRDoctor needs it".into() });
    let (ovr_ok, ovr_detail) = openvr_routing_status();
    push("OpenVR routing", ovr_ok, ovr_detail);

    let steam_games = list_games(&cfg);
    push("Steam games detected", !steam_games.is_empty(), format!("{} installed title(s) in Steam libraries", steam_games.len()));
    let custom = load_custom_games();
    push("Custom games", !custom.is_empty(), format!("{} custom (non-store) game(s) in {}", custom.len(), custom_games_path().display()));

    let quests = detect_quest();
    if quests.is_empty() {
        push("Quest 3 over USB", false, "no adb device — OK for wireless WiVRn if headset + PC share Wi-Fi and the WiVRn client is open on the Quest".into());
    } else {
        for (d, model, device) in &quests {
            push("Quest device over USB", true, format!("{} {} model={} device={} {}", d.serial, d.state, model.clone().unwrap_or_default(), device.clone().unwrap_or_default(), if is_quest3(model, device) { "(Quest 3 ✓)" } else { "" }));
        }
    }
    out
}

fn doctor() -> i32 {
    println!("VRDoctor — doctor");
    for c in run_checks() {
        println!("  [{}] {:<24} {}", if c.ok { " OK " } else { "MISS" }, c.label, c.detail);
    }
    println!("\nFirewall reminder: allow UDP 5353 (mDNS) and TCP/UDP 9757 (WiVRn). ALVR uses TCP/UDP 9943-9944.");
    println!("Next: vrdoctor setup   then   vrdoctor gui   or   vrdoctor launch --game <appid|name>");
    let checks = run_checks();
    let distro_ok = matches!(current_distro().family, Family::Arch | Family::Fedora);
    let backend_ok = checks.iter().any(|c| (c.label.starts_with("WiVRn") || c.label.starts_with("ALVR")) && c.ok);
    if distro_ok && backend_ok { 0 } else { 1 }
}

// ---------------------------------------------------------------- setup

pub(crate) fn setup_plan(distro: &Distro) -> Vec<String> { install_plan(&distro.family) }

/// The install plan for a chosen distro family — the installer asks the user
/// to pick Arch or Fedora, then installs accordingly using this plan.
pub(crate) fn setup_plan_for_family(family: &Family) -> Vec<String> {
    match family {
        Family::Arch => vec![
            "sudo pacman -S --needed steam android-tools avahi pipewire pipewire-pulse vulkan-headers libva-mesa-driver git base-devel".into(),
            "# WiVRn (native, preferred): AUR package wivrn-dashboard  — e.g. with yay:".into(),
            "yay -S wivrn-dashboard".into(),
            "# …or distro-agnostic Flatpak (bundles xrizer + OpenComposite):".into(),
            "flatpak install -y flathub io.github.wivrn.wivrn".into(),
            "sudo systemctl enable --now avahi-daemon".into(),
            "# Optional, for 2D games / desktop in VR: yay -S wayvr  (or wlx-overlay-s)".into(),
            "# Optional fallback: yay -S alvr".into(),
        ],
        Family::Fedora => vec![
            "# Enable RPM Fusion (needed for Steam + full codecs):".into(),
            "sudo dnf install https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm https://mirrors.rpmfusion.org/nonfree/fedora/rpmfusion-nonfree-release-$(rpm -E %fedora).noarch.rpm".into(),
            "sudo dnf install steam android-tools avahi pipewire vulkan-headers libva-devel wivrn".into(),
            "sudo systemctl enable --now avahi-daemon".into(),
            "# Optional, for 2D games / desktop in VR: WayVR — see https://github.com/galister/WayVR (not in Fedora repos yet)".into(),
            "# Optional fallback: install ALVR streamer tarball from https://github.com/alvr-org/ALVR/releases".into(),
        ],
        _ => vec![
            "# Scripted setup targets Arch and Fedora.".into(),
            "# On other distros use the Flatpak: flatpak install -y flathub io.github.wivrn.wivrn".into(),
            "flatpak install -y flathub io.github.wivrn.wivrn".into(),
        ],
    }
}

/// Marker written after the installer has run once (the AppImage opens the
/// installer on first run, the main GUI afterwards).
pub(crate) fn install_marker_path() -> PathBuf { config_dir().join("installed-v1") }
pub(crate) fn install_done() -> bool { install_marker_path().exists() }
pub(crate) fn mark_installed() -> std::io::Result<()> {
    if let Some(p) = install_marker_path().parent() { fs::create_dir_all(p)?; }
    fs::write(install_marker_path(), "installed\n")
}

/// Write the chosen distro's install plan as a runnable shell script and
/// open it in a terminal (so sudo can prompt), falling back to pkexec.
pub(crate) fn run_installer_terminal(family: &Family) -> Result<String, String> {
    let cache = env::var_os("XDG_CACHE_HOME").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".cache")).join("vrdoctor");
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let script = cache.join("install-backend.sh");
    let mut body = String::from("#!/usr/bin/env bash\nset -x\n# VRDoctor backend installer\n");
    for line in install_plan(family) {
        let t = line.trim();
        if t.starts_with('#') { body.push_str(&format!("# {}\n", &t[1..])); } else { body.push_str(t); body.push('\n'); }
    }
    body.push_str("echo\necho 'VRDoctor backend install finished. You can close this terminal.'\n");
    fs::write(&script, body).map_err(|e| e.to_string())?;
    ensure_executable(&script);
    for term in ["konsole", "gnome-terminal", "kitty", "alacritty", "xfce4-terminal", "xterm"] {
        if let Some(t) = which(term) {
            let args: Vec<String> = match term {
                "gnome-terminal" | "xfce4-terminal" => vec!["--".into(), "bash".into(), script.display().to_string()],
                _ => vec!["-e".into(), "bash".into(), script.display().to_string()],
            };
            if Command::new(t).args(&args).spawn().is_ok() {
                let _ = mark_installed();
                return Ok(format!("Installer opened in {term} — enter your password there when sudo asks. Script: {}", script.display()));
            }
        }
    }
    if let Some(pk) = which("pkexec") {
        if Command::new(pk).arg("bash").arg(&script).spawn().is_ok() {
            let _ = mark_installed();
            return Ok("Installer started with pkexec — approve the admin prompt.".into());
        }
    }
    Err(format!("No terminal or pkexec found. Run the installer script manually:\n  bash {}", script.display()))
}

pub(crate) fn quest_setup_steps() -> Vec<String> {
    vec![
        "1. Install the WiVRn client on the headset (Meta Store / SideQuest — version MUST match the PC server).".into(),
        "   For ALVR instead, install the ALVR client app on the headset.".into(),
        "2. Put headset and PC on the same 5 GHz / 6 GHz Wi-Fi (router wired to PC is best), or use USB.".into(),
        "3. Open the WiVRn client on the Quest, then launch a game here.".into(),
    ]
}

fn do_setup(apply: bool) {
    let distro = current_distro();
    println!("Setup plan for {}:\n", distro.pretty);
    for step in setup_plan(&distro) { println!("  {step}"); }
    println!("\nOn the Quest 3 itself (one-time):");
    for step in quest_setup_steps() { println!("  {step}"); }
    if !apply { println!("\nDry run only. Re-run with --apply to execute the non-comment commands above (sudo will prompt)."); return; }
    for step in setup_plan(&distro) {
        let cmd = step.trim();
        if cmd.is_empty() || cmd.starts_with('#') { continue; }
        println!("\n$ {cmd}");
        match Command::new("sh").arg("-c").arg(cmd).status() {
            Ok(s) if s.success() => println!("  ok"),
            Ok(s) => println!("  exited with {s} — continuing"),
            Err(e) => println!("  failed: {e}"),
        }
    }
    let _ = save_default_config();
}

// ---------------------------------------------------------------- launch

pub(crate) fn wivrn_runtime_json() -> Option<PathBuf> {
    for c in ["/usr/share/openxr/1/openxr_wivrn.json", "/usr/local/share/openxr/1/openxr_wivrn.json", "/app/share/openxr/1/openxr_wivrn.json"] {
        if Path::new(c).exists() { return Some(PathBuf::from(c)); }
    }
    let home = PathBuf::from(env::var("HOME").unwrap_or_default());
    let fp = home.join(".local/share/flatpak/app/io.github.wivrn.wivrn/current/active/files/share/openxr/1/openxr_wivrn.json");
    if fp.exists() { return Some(fp); }
    None
}

pub(crate) fn start_backend(backend: &str) -> String {
    match backend {
        "wivrn" => {
            if is_running("wivrn-server") { return "WiVRn server already running.".into(); }
            if which("wivrn-dashboard").is_some() {
                let _ = Command::new("wivrn-dashboard").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                "Starting WiVRn dashboard (it starts wivrn-server on connect)…".into()
            } else if which("wivrn-server").is_some() {
                let _ = Command::new("wivrn-server").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                "Starting wivrn-server…".into()
            } else if flatpak_installed("io.github.wivrn.wivrn") {
                let _ = Command::new("flatpak").args(["run", "io.github.wivrn.wivrn"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                "Starting WiVRn via Flatpak…".into()
            } else { "WiVRn not installed. Run: vrdoctor setup".into() }
        }
        "alvr" => {
            if let Some(bin) = which("alvr_dashboard").or_else(|| which("ALVR")) {
                let _ = Command::new(bin).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                "Starting ALVR dashboard…".into()
            } else { "ALVR not installed. See: vrdoctor setup".into() }
        }
        "steamlink" => "Steam Link: start SteamVR on the PC, then open the Steam Link app on the Quest 3 and pair.".into(),
        other => format!("Unknown backend '{other}' (use wivrn | alvr | steamlink)"),
    }
}

fn apply_vr_env(cmd: &mut Command, backend: &str) {
    if backend == "wivrn" {
        if let Some(json) = wivrn_runtime_json() { cmd.env("XR_RUNTIME_JSON", json); }
        // Let Proton/Steam's pressure-vessel sandbox reach the WiVRn socket.
        // No SteamVR is involved anywhere in this path: OpenVR games are
        // translated by xrizer (bundled with / managed by WiVRn ≥ 0.23).
        if let Ok(rt) = env::var("XDG_RUNTIME_DIR") {
            cmd.env("PRESSURE_VESSEL_FILESYSTEMS_RW", format!("{rt}/wivrn/comp_ipc"));
        }
    }
}

pub(crate) fn launch_steam_appid(appid: u64, backend: &str) -> Result<String, String> {
    let msg = start_backend(backend);
    let flavour = detect_steam_flavour(&load_config());
    let mut cmd = steam_launch_command(flavour, appid);
    apply_vr_env(&mut cmd, backend);
    cmd.spawn().map(|_| format!("{msg}\nLaunched Steam AppID {appid} via {}. Confirm the {backend} client is connected in the headset.", flavour.label()))
        .map_err(|e| format!("Failed to launch Steam: {e}"))
}

/// Build a Proton/Wine command for a Windows file (.exe / .bat / .msi).
fn windows_command(game: &CustomGame, cfg: &Config, kind: GameKind, args: &[String]) -> Result<Command, String> {
    let proton = if !game.proton.is_empty() { Some(PathBuf::from(&game.proton)) } else { find_proton(cfg) };
    if let Some(proton) = proton {
        let compat = config_dir().join("compatdata").join(sanitize_name(&game.name));
        let _ = fs::create_dir_all(&compat);
        let mut cmd = Command::new(&proton);
        cmd.arg("run").arg(&game.path).args(args);
        cmd.env("STEAM_COMPAT_DATA_PATH", &compat);
        let root = steam_roots(cfg).into_iter().next()
            .unwrap_or_else(|| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".local/share/Steam"));
        cmd.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", root);
        return Ok(cmd);
    }
    if let Some(wine) = which("wine") {
        let mut cmd = Command::new(wine);
        match kind {
            GameKind::WindowsBatch => { cmd.arg("cmd").arg("/c").arg(&game.path).args(args); }
            GameKind::WindowsInstaller => { cmd.arg("msiexec").arg("/i").arg(&game.path).args(args); }
            _ => { cmd.arg(&game.path).args(args); }
        }
        return Ok(cmd);
    }
    Err(format!("'{}' is a Windows file but no Proton or Wine was found. Install the game once via Steam (Add a Non-Steam Game) so Proton exists, set a Proton path in the GUI/CLI, or install Wine.", game.name))
}

fn interpreter_command(game: &CustomGame, kind: GameKind, args: &[String]) -> Result<Command, String> {
    let (prog, hint): (&str, &str) = match kind {
        GameKind::ShellScript => ("bash", "bash"),
        GameKind::Python => ("python3", "python3"),
        GameKind::Perl => ("perl", "perl"),
        GameKind::Ruby => ("ruby", "ruby"),
        GameKind::Node => ("node", "nodejs"),
        GameKind::Jar => ("java", "a Java runtime (Arch: jre-openjdk, Fedora: java-latest-openjdk)"),
        GameKind::Love => ("love", "love (LÖVE)"),
        _ => unreachable!(),
    };
    let bin = which(prog).ok_or_else(|| format!("'{}' needs {hint}, which was not found on this system.", game.name))?;
    let mut cmd = Command::new(bin);
    if kind == GameKind::Jar { cmd.arg("-jar"); }
    cmd.arg(&game.path).args(args);
    Ok(cmd)
}

pub(crate) fn launch_custom_game(game: &CustomGame, cfg: &Config) -> Result<String, String> {
    if game.mode == "2d" { return launch_custom_game_impl(game, cfg, true); }
    launch_custom_game_impl(game, cfg, false)
}

/// 2D mode: the game renders flat and shows on the big virtual screen in VR
/// (desktop-in-VR overlay), with the VRDoctor bar under the screen.
pub(crate) fn launch_custom_game_2d(game: &CustomGame, cfg: &Config) -> Result<String, String> {
    launch_custom_game_impl(game, cfg, true)
}

fn launch_custom_game_impl(game: &CustomGame, cfg: &Config, mode2d: bool) -> Result<String, String> {
    let backend = if game.backend.is_empty() { cfg.backend.clone() } else { game.backend.clone() };
    if !game.path.exists() {
        return Err(format!("Custom game '{}': file not found at {}", game.name, game.path.display()));
    }
    let kind = detect_kind(&game.path);

    // Kinds that don't stream from the PC at all — handle them directly.
    match kind {
        GameKind::AndroidApk => {
            // A Quest-native game: sideload it onto the headset instead of streaming.
            if which("adb").is_none() { return Err("Sideloading an .apk needs adb (Arch: android-tools, Fedora: android-tools).".into()); }
            let devices = detect_quest();
            if devices.is_empty() { return Err("No Quest found over USB. Connect the Quest 3, enable Developer Mode + USB debugging, and accept the prompt in the headset.".into()); }
            let out = Command::new("adb").args(["install", "-r"]).arg(&game.path).output()
                .map_err(|e| format!("adb failed: {e}"))?;
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            return if out.status.success() {
                Ok(format!("Installed '{}' on your Quest 3 (it's a native Quest game, so it runs on the headset itself — find it under Unknown Sources / App Library).", game.name))
            } else { Err(format!("adb install failed: {}", text.trim())) };
        }
        GameKind::FlatpakBundle | GameKind::FlatpakRef => {
            if which("flatpak").is_none() { return Err("Installing a Flatpak needs flatpak (Arch: flatpak, Fedora: preinstalled).".into()); }
            let out = Command::new("flatpak").args(["install", "--user", "-y"]).arg(&game.path).output()
                .map_err(|e| format!("flatpak failed: {e}"))?;
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            return if out.status.success() {
                Ok(format!("Installed '{}' as a Flatpak. Its app ID isn't known from the bundle alone — after installing, add the installed app back here by its executable if you want to stream it, or launch it from your desktop.", game.name))
            } else { Err(format!("flatpak install failed: {}", text.trim())) };
        }
        GameKind::Archive | GameKind::Directory => {
            // Games added before resolution existed, or launched by path: resolve now.
            let (resolved, note) = resolve_game_target(game)?;
            let mut g = resolved;
            if g.backend.is_empty() { g.backend = backend.clone(); }
            let inner = launch_custom_game_impl(&g, cfg, mode2d)?;
            return Ok(if note.is_empty() { inner } else { format!("{note}\n{inner}") });
        }
        GameKind::Unknown => {
            return Err(format!("VRDoctor doesn't know how to run {} — supported: {SUPPORTED_KINDS}.", game.path.display()));
        }
        _ => {}
    }

    let msg = if mode2d {
        format!("{}\n{}\n{}", start_backend(&backend), start_overlay(), start_bar())
    } else { start_backend(&backend) };
    let args = split_args(&game.args);
    let mut cmd = match kind {
        GameKind::WindowsExe | GameKind::WindowsBatch | GameKind::WindowsInstaller => windows_command(game, cfg, kind, &args)?,
        GameKind::ShellScript | GameKind::Python | GameKind::Perl | GameKind::Ruby | GameKind::Node | GameKind::Jar | GameKind::Love => interpreter_command(game, kind, &args)?,
        GameKind::DesktopEntry => {
            let content = fs::read_to_string(&game.path).map_err(|e| format!("Could not read {}: {e}", game.path.display()))?;
            let (prog, mut dargs) = parse_desktop_exec(&content).ok_or_else(|| format!("No Exec= line found in {}", game.path.display()))?;
            dargs.extend(args.clone());
            let mut c = Command::new(&prog);
            c.args(&dargs);
            c
        }
        // NativeExecutable, AppImage, and anything else runnable: run it directly.
        _ => {
            ensure_executable(&game.path);
            let mut c = Command::new(&game.path);
            c.args(&args);
            c
        }
    };
    if let Some(w) = &game.workdir { cmd.current_dir(w); }
    else if let Some(parent) = game.path.parent() { if kind != GameKind::DesktopEntry { cmd.current_dir(parent); } }
    if !mode2d { apply_vr_env(&mut cmd, &backend); }
    let how = match kind {
        GameKind::WindowsInstaller => " (installer — follow its setup, then add the installed game)",
        GameKind::DesktopEntry => " (via its .desktop Exec line)",
        _ => "",
    };
    let screen = if mode2d { " on the big screen in VR (2D mode — look at the desktop panel; the VRDoctor bar sits under it)" } else { "" };
    cmd.spawn().map(|_| format!("{msg}\nLaunched custom game '{}' ({:?}, {}) via {backend}{how}{screen}.", game.name, kind, game.path.display()))
        .map_err(|e| format!("Failed to launch '{}': {e}", game.name))
}

// ------------------------------------------------------- OpenVR routing (no SteamVR)

// OpenVR games find their runtime through ~/.config/openvr/openvrpaths.vrpath.
// A (broken) SteamVR install registers itself there and can hijack games that
// should be going to WiVRn/xrizer. VRDoctor can detect and repair that.

pub(crate) fn openvrpaths_path() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_default()).join(".config/openvr/openvrpaths.vrpath")
}

/// Extract the "runtime" array entries from a vrpath file (best-effort, no JSON parser).
pub(crate) fn vrpath_runtimes(content: &str) -> Vec<String> {
    let Some(pos) = content.find("\"runtime\"") else { return vec![] };
    let rest = &content[pos..];
    let (Some(lb), Some(rb)) = (rest.find('['), rest.find(']')) else { return vec![] };
    let inner = &rest[lb + 1..rb];
    let mut out = vec![];
    let mut cur = String::new();
    let mut in_str = false;
    let mut esc = false;
    for ch in inner.chars() {
        if esc { cur.push(ch); esc = false; continue; }
        match ch {
            '\\' if in_str => esc = true,
            '"' => { if in_str { out.push(std::mem::take(&mut cur)); } in_str = !in_str; }
            c if in_str => cur.push(c),
            _ => {}
        }
    }
    out
}

pub(crate) fn build_vrpath(runtime: &Path) -> String {
    format!("{{\n\t\"config\" : [],\n\t\"external_drivers\" : null,\n\t\"jsonid\" : \"vrpathreg\",\n\t\"log\" : [],\n\t\"runtime\" : [\n\t\t\"{}\"\n\t],\n\t\"version\" : 1\n}}", runtime.display())
}

/// Find an xrizer / OpenComposite runtime on this system (the OpenVR→OpenXR
/// translators that replace SteamVR for WiVRn).
pub(crate) fn find_openvr_runtime() -> Option<PathBuf> {
    let home = PathBuf::from(env::var("HOME").unwrap_or_default());
    let mut candidates = vec![
        home.join(".local/share/xrizer"),
        PathBuf::from("/usr/share/xrizer"),
        PathBuf::from("/usr/local/share/xrizer"),
        home.join(".local/share/OpenComposite"),
        PathBuf::from("/usr/share/opencomposite"),
        PathBuf::from("/usr/local/share/opencomposite"),
    ];
    // WiVRn Flatpak bundles OpenComposite/xrizer.
    for base in [home.join(".local/share/flatpak/app/io.github.wivrn.wivrn/current/active/files/share"),
                 PathBuf::from("/var/lib/flatpak/app/io.github.wivrn.wivrn/current/active/files/share")] {
        candidates.push(base.join("opencomposite"));
        candidates.push(base.join("xrizer"));
    }
    candidates.into_iter().find(|p| p.is_dir())
}

pub(crate) fn openvr_routing_status() -> (bool, String) {
    let p = openvrpaths_path();
    if !p.exists() {
        return (true, "no openvrpaths.vrpath — WiVRn manages OpenVR routing itself when your headset connects".into());
    }
    let content = fs::read_to_string(&p).unwrap_or_default();
    let runtimes = vrpath_runtimes(&content);
    if runtimes.is_empty() { return (true, format!("{} lists no runtime — WiVRn will manage it", p.display())); }
    let lower: Vec<String> = runtimes.iter().map(|r| r.to_lowercase()).collect();
    let good = lower.iter().any(|r| r.contains("xrizer") || r.contains("opencomposite") || r.contains("wivrn") || r.contains("monado"));
    let only_steamvr = lower.iter().all(|r| r.contains("steamvr") || r.contains("steamapps/common/steamvr"));
    if good {
        (true, format!("OpenVR games route to {} (not SteamVR)", runtimes.join(", ")))
    } else if only_steamvr {
        (false, format!("openvrpaths.vrpath points ONLY at SteamVR ({}) — a broken SteamVR can hijack your OpenVR games. Fix: vrdoctor fix-openvr", runtimes.join(", ")))
    } else {
        (true, format!("runtime(s): {}", runtimes.join(", ")))
    }
}

/// Repair OpenVR routing so games reach WiVRn/xrizer instead of SteamVR.
/// Backs the original file up to openvrpaths.vrpath.vrdoctor-bak first.
pub(crate) fn fix_openvr() -> Result<String, String> {
    let p = openvrpaths_path();
    let backup = p.with_file_name("openvrpaths.vrpath.vrdoctor-bak");
    if p.exists() && !backup.exists() {
        fs::copy(&p, &backup).map_err(|e| format!("Could not back up {}: {e}", p.display()))?;
    }
    if let Some(runtime) = find_openvr_runtime() {
        if let Some(parent) = p.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        // Remove a previous read-only lock so we can write.
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; if let Ok(m) = fs::metadata(&p) { let mut perms = m.permissions(); perms.set_mode(0o644); let _ = fs::set_permissions(&p, perms); } }
        fs::write(&p, build_vrpath(&runtime)).map_err(|e| e.to_string())?;
        // Lock it: SteamVR re-adds itself to this file if it can write to it.
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; if let Ok(m) = fs::metadata(&p) { let mut perms = m.permissions(); perms.set_mode(0o444); let _ = fs::set_permissions(&p, perms); } }
        Ok(format!("OpenVR now routes to {} ({}).\nThe file is locked read-only so SteamVR can't hijack it back. Backup of the original: {}\nUndo any time with: vrdoctor fix-openvr --undo", runtime.display(), p.display(), backup.display()))
    } else if p.exists() {
        // No translator found: removing the file is the documented workaround —
        // WiVRn ≥ 0.23 manages the paths itself when the headset connects.
        fs::remove_file(&p).map_err(|e| e.to_string())?;
        Ok(format!("No xrizer/OpenComposite runtime found, so the SteamVR-pointing {} was removed (backup: {}).\nWiVRn manages OpenVR routing itself when your headset connects. For older WiVRn, install xrizer (Arch: yay -S xrizer) and re-run `vrdoctor fix-openvr`.\nUndo any time with: vrdoctor fix-openvr --undo", p.display(), backup.display()))
    } else {
        Ok("Nothing to fix: no openvrpaths.vrpath exists, so nothing is routing OpenVR games to SteamVR. WiVRn manages routing when your headset connects.".into())
    }
}

pub(crate) fn fix_openvr_undo() -> Result<String, String> {
    let p = openvrpaths_path();
    let backup = p.with_file_name("openvrpaths.vrpath.vrdoctor-bak");
    if !backup.exists() { return Err(format!("No backup found at {}", backup.display())); }
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; if let Ok(m) = fs::metadata(&p) { let mut perms = m.permissions(); perms.set_mode(0o644); let _ = fs::set_permissions(&p, perms); } }
    fs::copy(&backup, &p).map_err(|e| e.to_string())?;
    Ok(format!("Restored {} from {}", p.display(), backup.display()))
}

// ------------------------------------------------------- 2D mode / overlay / bar

/// Start the desktop-in-VR overlay used for 2D games and the desktop panel.
pub(crate) fn start_overlay() -> String {
    if is_running("wayvr") || is_running("wlx-overlay-s") { return "2D overlay already running.".into(); }
    if let Some(w) = which("wayvr") {
        let _ = Command::new(w).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        "Starting WayVR (your desktop + 2D games appear on the big screen in VR)…".into()
    } else if let Some(w) = which("wlx-overlay-s") {
        let _ = Command::new(w).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        "Starting wlx-overlay-s (desktop in VR)…".into()
    } else {
        "2D overlay not installed — the game will still launch on your desktop, but to see it in the headset install WayVR (Arch: yay -S wayvr · Fedora: see WayVR releases) or wlx-overlay-s. With SteamVR, its theatre/desktop view also works.".into()
    }
}

/// Start the Quest-style VRDoctor bar (a slim dock that sits at the bottom
/// of the desktop, so inside the VR desktop panel it sits under the screen).
pub(crate) fn start_bar() -> String {
    if is_running("vrdoctor") && env::var("VRDOCTOR_BAR_STARTED").is_ok() { return "VRDoctor bar already running.".into(); }
    if let Ok(exe) = env::current_exe() {
        let spawned = Command::new(exe).arg("bar").env("VRDOCTOR_BAR_STARTED", "1")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        if spawned.is_ok() { return "VRDoctor bar started (bottom of your desktop — under the screen in VR).".into(); }
    }
    "Could not start the VRDoctor bar (run `vrdoctor bar` manually).".into()
}

/// Launch a Steam game flat, for the big screen in VR.
pub(crate) fn launch_steam_2d(appid: u64, backend: &str) -> Result<String, String> {
    let msg = format!("{}\n{}\n{}", start_backend(backend), start_overlay(), start_bar());
    let flavour = detect_steam_flavour(&load_config());
    steam_launch_command(flavour, appid).spawn()
        .map(|_| format!("{msg}\nLaunched Steam AppID {appid} in 2D mode via {} — it will appear on the desktop panel / big screen in VR.", flavour.label()))
        .map_err(|e| format!("Failed to launch Steam: {e}"))
}

// ------------------------------------------------------- VR Home environment

pub(crate) fn home_dir() -> PathBuf { config_dir().join("home") }
pub(crate) fn home_panorama_path() -> PathBuf { home_dir().join("vrdoctor-home-dawn.png") }
pub(crate) fn home_panorama_path_for(theme: &str) -> PathBuf { home_dir().join(format!("vrdoctor-home-{theme}.png")) }

/// VR Home environment themes — deliberately varied, not just darkness:
/// bright dawn, a green meadow, an ocean coast, a warm sunset, a starry
/// (but not black) night, a white studio, and a cozy warm room.
pub(crate) const HOME_THEMES: &[&str] = &["dawn", "meadow", "ocean", "sunset", "night", "studio", "cozy"];

struct Palette {
    sky_top: (f32, f32, f32), sky_horizon: (f32, f32, f32),
    floor_near: (f32, f32, f32), floor_far: (f32, f32, f32),
    glow: (f32, f32, f32), glow_pos: (f32, f32), glow_strength: f32,
    panel_add: f32, stars: bool,
    /// Sun/moon disc: (radius, colour added at the disc).
    disc: Option<(f32, (f32, f32, f32))>,
    /// Distant ridge/hills silhouette colour just above the horizon.
    ridge: Option<(f32, f32, f32)>,
}

fn palette(theme: &str) -> Palette {
    match theme {
        "meadow" => Palette { sky_top: (0.55, 0.78, 0.98), sky_horizon: (0.92, 0.98, 0.90), floor_near: (0.62, 0.82, 0.52), floor_far: (0.30, 0.55, 0.30), glow: (0.25, 0.20, 0.05), glow_pos: (0.5, 0.34), glow_strength: 3.5, panel_add: -0.04, stars: false, disc: Some((0.14, (0.32, 0.28, 0.10))), ridge: Some((0.16, 0.32, 0.16)) },
        "ocean" => Palette { sky_top: (0.45, 0.75, 0.95), sky_horizon: (0.85, 0.97, 0.98), floor_near: (0.45, 0.80, 0.85), floor_far: (0.10, 0.45, 0.65), glow: (0.20, 0.18, 0.10), glow_pos: (0.5, 0.36), glow_strength: 3.5, panel_add: -0.06, stars: false, disc: Some((0.15, (0.30, 0.26, 0.12))), ridge: Some((0.08, 0.30, 0.42)) },
        "sunset" => Palette { sky_top: (0.45, 0.35, 0.65), sky_horizon: (1.05, 0.55, 0.30), floor_near: (0.55, 0.38, 0.35), floor_far: (0.25, 0.16, 0.22), glow: (0.45, 0.22, 0.02), glow_pos: (0.5, 0.48), glow_strength: 2.2, panel_add: 0.06, stars: false, disc: Some((0.24, (0.55, 0.25, 0.02))), ridge: Some((0.30, 0.14, 0.24)) },
        "night" => Palette { sky_top: (0.04, 0.07, 0.20), sky_horizon: (0.16, 0.20, 0.42), floor_near: (0.14, 0.17, 0.28), floor_far: (0.05, 0.06, 0.12), glow: (0.18, 0.20, 0.28), glow_pos: (0.72, 0.22), glow_strength: 5.0, panel_add: 0.08, stars: true, disc: Some((0.10, (0.35, 0.38, 0.45))), ridge: Some((0.05, 0.07, 0.16)) },
        "studio" => Palette { sky_top: (0.97, 0.97, 0.98), sky_horizon: (1.0, 1.0, 1.0), floor_near: (0.93, 0.93, 0.94), floor_far: (0.78, 0.78, 0.80), glow: (0.06, 0.06, 0.06), glow_pos: (0.5, 0.30), glow_strength: 3.0, panel_add: -0.05, stars: false, disc: None, ridge: None },
        "cozy" => Palette { sky_top: (0.92, 0.72, 0.52), sky_horizon: (1.02, 0.85, 0.62), floor_near: (0.62, 0.44, 0.30), floor_far: (0.36, 0.24, 0.16), glow: (0.30, 0.18, 0.04), glow_pos: (0.5, 0.40), glow_strength: 3.0, panel_add: 0.05, stars: false, disc: Some((0.18, (0.30, 0.18, 0.03))), ridge: None },
        // "dawn" — the default: bright, calm, Frame-like but lighter.
        _ => Palette { sky_top: (0.80, 0.88, 0.97), sky_horizon: (1.0, 0.99, 0.94), floor_near: (0.88, 0.90, 0.93), floor_far: (0.68, 0.71, 0.76), glow: (0.17, 0.12, 0.04), glow_pos: (0.5, 0.38), glow_strength: 3.5, panel_add: -0.04, stars: false, disc: Some((0.16, (0.20, 0.15, 0.05))), ridge: None },
    }
}

/// Deterministic pseudo-random star field for the night theme.
fn is_star(x: usize, y: usize) -> bool {
    let mut n = (x as u64).wrapping_mul(0x9E3779B97F4A7C15) ^ (y as u64).wrapping_mul(0xC2B2AE3D27D4EB4F);
    n ^= n >> 29; n = n.wrapping_mul(0xBF58476D1CE4E5B9); n ^= n >> 32;
    n % 997 < 3
}

/// Render the default (dawn) panorama. Kept for compatibility.
pub(crate) fn render_panorama(path: &Path, brightness: f32) -> Result<(), String> {
    render_panorama_theme(path, "dawn", brightness)
}

/// Procedurally render a VRDoctor Home panorama in the given theme.
/// `brightness` 1.10 by default: a touch brighter, not much.
pub(crate) fn render_panorama_theme(path: &Path, theme: &str, brightness: f32) -> Result<(), String> {
    let pal = palette(theme);
    let (w, h) = (2048usize, 1024usize);
    let mut px = vec![0u8; w * h * 3];
    let clamp = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
    let lerp = |a: (f32, f32, f32), b: (f32, f32, f32), t: f32| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t);
    // Floating panel rectangles (x0, x1, y0, y1 in image fractions).
    let panels = [(0.30f32, 0.46f32, 0.30f32, 0.47f32), (0.54, 0.70, 0.30, 0.47), (0.42, 0.58, 0.16, 0.26)];
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            let (mut r, mut g, mut b);
            if v < 0.54 {
                let t = v / 0.54;
                let c = lerp(pal.sky_top, pal.sky_horizon, t);
                r = c.0; g = c.1; b = c.2;
                // Sun / moon glow.
                let dx = (u - pal.glow_pos.0) * 2.0; let dy = (v - pal.glow_pos.1) * 2.6;
                let glow = (-(dx * dx + dy * dy) * pal.glow_strength).exp();
                r += glow * pal.glow.0; g += glow * pal.glow.1; b += glow * pal.glow.2;
                // Crisp sun / moon disc inside the glow.
                if let Some((radius, dc)) = pal.disc {
                    let d = (dx * dx + dy * dy).sqrt();
                    let edge = ((radius - d) / (radius * 0.35)).clamp(0.0, 1.0);
                    r += dc.0 * edge; g += dc.1 * edge; b += dc.2 * edge;
                }
                if pal.stars && is_star(x, y) { r += 0.55; g += 0.55; b += 0.60; }
                // Distant ridge silhouette just above the horizon.
                if let Some(rc) = pal.ridge {
                    let ridge_y = 0.475 + 0.028 * (u * 18.85 + 1.7).sin() + 0.014 * (u * 50.27 + 0.4).sin();
                    if v > ridge_y {
                        let m = ((v - ridge_y) / 0.015).clamp(0.0, 1.0) * 0.7;
                        r += (rc.0 - r) * m; g += (rc.1 - g) * m; b += (rc.2 - b) * m;
                    }
                }
                // Faint floating panels, with a brighter rim so they read as panels.
                for (x0, x1, y0, y1) in panels {
                    if u > x0 && u < x1 && v > y0 && v < y1 { r += pal.panel_add; g += pal.panel_add; b += pal.panel_add; }
                    else if u > x0 - 0.0035 && u < x1 + 0.0035 && v > y0 - 0.0035 && v < y1 + 0.0035 {
                        let e = pal.panel_add * 2.2; r += e; g += e; b += e;
                    }
                }
            } else {
                let t = (v - 0.54) / 0.46;
                let c = lerp(pal.floor_near, pal.floor_far, t);
                r = c.0; g = c.1; b = c.2;
                // Soft glow reflection (water / polished floor).
                if pal.ridge.is_some() {
                    let rx = (u - pal.glow_pos.0) * 2.0;
                    let refl = (-rx * rx * 9.0).exp() * (1.0 - t) * 0.12;
                    r += refl * pal.glow.0 * 2.0; g += refl * pal.glow.1 * 2.0; b += refl * pal.glow.2 * 2.0;
                }
                // Subtle radial guides on the floor.
                let dx = (u - 0.5) * 2.0;
                let ring = ((dx * dx * 4.0 + t * t * 9.0).sqrt() * 6.0).fract();
                if ring < 0.06 { r -= 0.03; g -= 0.03; b -= 0.03; }
            }
            // Ordered-noise dither: smooths gradient banding in the headset.
            let mut hn = (x as u64).wrapping_mul(0x85EBCA6B) ^ (y as u64).wrapping_mul(0x27D4EB2D);
            hn ^= hn >> 15; hn = hn.wrapping_mul(0x2C1B3C6D); hn ^= hn >> 12;
            let dither = ((hn % 1024) as f32 / 1024.0 - 0.5) * 0.012;
            let i = (y * w + x) * 3;
            px[i] = clamp((r + dither) * brightness); px[i + 1] = clamp((g + dither) * brightness); px[i + 2] = clamp((b + dither) * brightness);
        }
    }
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let file = fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&px).map_err(|e| e.to_string())?;
    Ok(())
}

/// Set SteamVR's compositor background ("steamvr.background") in a
/// steamvr.vrsettings document. Pure function so it can be unit-tested.
pub(crate) fn set_vrsettings_background(content: &str, image_path: &str) -> String {
    let escaped = image_path.replace('\\', "\\\\").replace('"', "\\\"");
    if let Some(pos) = content.find("\"background\"") {
        // Replace the value of the first "background" key.
        if let Some(colon) = content[pos..].find(':') {
            let after = &content[pos + colon + 1..];
            if let Some(q1) = after.find('"') {
                if let Some(q2) = after[q1 + 1..].find('"') {
                    let start = pos + colon + 1 + q1;
                    let end = pos + colon + 1 + q1 + 1 + q2 + 1;
                    return format!("{}\"{}\"{}", &content[..start], escaped, &content[end..]);
                }
            }
        }
    }
    if let Some(sv) = content.find("\"steamvr\"") {
        if let Some(brace) = content[sv..].find('{') {
            let at = sv + brace + 1;
            return format!("{}\n\t\t\"background\" : \"{}\",{}", &content[..at], escaped, &content[at..]);
        }
    }
    format!("{{\n\t\"steamvr\" : {{\n\t\t\"background\" : \"{}\"\n\t}}\n}}", escaped)
}

/// Generate and install a VR Home panorama. Nothing here needs SteamVR:
/// the panorama file itself is the environment (WayVR/desktop backdrop on
/// the WiVRn route). Setting it as the SteamVR background is an optional
/// extra that only happens if a SteamVR install already exists.
pub(crate) fn home_install(theme: &str, brightness: f32) -> Result<String, String> {
    if !HOME_THEMES.contains(&theme) {
        return Err(format!("Unknown theme '{theme}'. Themes: {}", HOME_THEMES.join(", ")));
    }
    let pano = home_panorama_path_for(theme);
    render_panorama_theme(&pano, theme, brightness)?;
    let mut notes = vec![format!("VR Home '{theme}' panorama ready: {} (brightness {:.0}%)", pano.display(), brightness * 100.0)];
    notes.push("Use it as your desktop wallpaper / WayVR backdrop and it surrounds your panel in VR — no SteamVR involved.".into());
    let cfg = load_config();
    for root in steam_roots(&cfg) {
        let settings = root.join("config/steamvr.vrsettings");
        if settings.parent().map(|p| p.is_dir()).unwrap_or(false) && root.join("steamapps/common/SteamVR").is_dir() {
            let old = fs::read_to_string(&settings).unwrap_or_else(|_| "{\n\t\"steamvr\" : {\n\t}\n}".to_string());
            let new = set_vrsettings_background(&old, &pano.display().to_string());
            if fs::write(&settings, new).is_ok() {
                notes.push(format!("Optional extra: also set as the SteamVR background in {} (only relevant if you ever use the ALVR/Steam Link route).", settings.display()));
            }
        }
        let bgdir = root.join("steamapps/common/SteamVR/resources/backgrounds");
        if bgdir.is_dir() { let _ = fs::copy(&pano, bgdir.join(format!("vrdoctor-home-{theme}.png"))); }
    }
    notes.push(format!("Themes: {}. Try: vrdoctor home --theme sunset — default is dawn at 1.10 brightness.", HOME_THEMES.join(", ")));
    Ok(notes.join("\n"))
}

pub(crate) fn launch_path(path: &Path, backend: &str) -> Result<String, String> {
    let game = CustomGame { name: path.file_name().unwrap_or_default().to_string_lossy().to_string(), path: path.to_path_buf(), ..Default::default() };
    let cfg = load_config();
    // Use the requested backend rather than the configured default.
    let mut g = game; g.backend = backend.to_string();
    launch_custom_game(&g, &cfg)
}

fn launch(args: &[String]) {
    let cfg = load_config();
    let mut backend = cfg.backend.clone();
    let mut game: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--backend" => { i += 1; if i < args.len() { backend = args[i].clone(); } }
            "--game" => { i += 1; if i < args.len() { game = Some(args[i].clone()); } }
            other if !other.starts_with('-') && game.is_none() => game = Some(other.to_string()),
            _ => {}
        }
        i += 1;
    }
    let Some(game) = game else {
        println!("{}", start_backend(&backend));
        println!("\nBackend ready. Put on the Quest 3, open the {} client there, then launch a game:", if backend == "alvr" { "ALVR" } else { "WiVRn" });
        println!("  vrdoctor launch --backend {backend} --game 546560   # Half-Life: Alyx");
        println!("  vrdoctor list-games");
        return;
    };

    // Resolution order: 1) custom (non-store) game by name, 2) Steam AppID,
    // 3) filesystem path, 4) Steam game by name.
    let custom = load_custom_games();
    if let Some(g) = custom.iter().find(|g| g.name.to_lowercase() == game.to_lowercase())
        .or_else(|| custom.iter().find(|g| g.name.to_lowercase().contains(&game.to_lowercase()))) {
        match launch_custom_game(g, &cfg) { Ok(m) => println!("{m}"), Err(e) => { eprintln!("{e}"); std::process::exit(1); } }
        return;
    }
    if let Ok(appid) = game.parse::<u64>() {
        match launch_steam_auto(appid, &backend, &cfg) { Ok(m) => println!("{m}"), Err(e) => { eprintln!("{e}"); std::process::exit(1); } }
        return;
    }
    if Path::new(&game).exists() {
        match launch_path(Path::new(&game), &backend) { Ok(m) => println!("{m}"), Err(e) => { eprintln!("{e}"); std::process::exit(1); } }
        return;
    }
    let games = list_games(&cfg);
    if let Some(g) = games.iter().find(|g| g.name.to_lowercase().contains(&game.to_lowercase())) {
        println!("Launching '{}' (AppID {}) …", g.name, g.appid);
        match launch_steam_auto(g.appid, &backend, &cfg) { Ok(m) => println!("{m}"), Err(e) => { eprintln!("{e}"); std::process::exit(1); } }
    } else {
        eprintln!("Could not resolve game '{game}'. Try `vrdoctor list-games`, or add it with `vrdoctor add-game --name \"…\" --path /path/to/game`.");
        std::process::exit(2);
    }
}

// ---------------------------------------------------------------- custom-game CLI

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

fn add_game_cli(args: &[String]) {
    let Some(name) = flag_value(args, "--name") else { eprintln!("add-game needs --name \"My Game\""); std::process::exit(2); };
    let Some(path) = flag_value(args, "--path") else { eprintln!("add-game needs --path /path/to/executable-or-.exe"); std::process::exit(2); };
    let game = CustomGame {
        name,
        path: PathBuf::from(path),
        args: flag_value(args, "--args").unwrap_or_default(),
        workdir: flag_value(args, "--workdir").map(PathBuf::from),
        backend: flag_value(args, "--backend").unwrap_or_default(),
        proton: flag_value(args, "--proton").unwrap_or_default(),
        source: String::new(),
        mode: flag_value(args, "--mode").unwrap_or_else(|| "vr".to_string()),
    };
    if !game.path.exists() { eprintln!("{} does not exist.", game.path.display()); std::process::exit(1); }
    match register_custom_game(game) {
        Ok((resolved, note, p)) => {
            println!("Added custom game '{}' → {} ({:?})", resolved.name, resolved.path.display(), detect_kind(&resolved.path));
            if !note.is_empty() { println!("{note}"); }
            println!("Saved in {}", p.display());
        }
        Err(e) => { eprintln!("{e}"); std::process::exit(1); }
    }
}

fn play2d(args: &[String]) {
    let cfg = load_config();
    let mut backend = cfg.backend.clone();
    let mut game: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--backend" => { i += 1; if i < args.len() { backend = args[i].clone(); } }
            "--game" => { i += 1; if i < args.len() { game = Some(args[i].clone()); } }
            other if !other.starts_with('-') && game.is_none() => game = Some(other.to_string()),
            _ => {}
        }
        i += 1;
    }
    let Some(game) = game else {
        println!("{}\n{}\n{}", start_backend(&backend), start_overlay(), start_bar());
        println!("\n2D mode ready: your desktop is the big screen in VR, with the VRDoctor bar under it. Launch a game with: vrdoctor play2d --game <name|appid|path>");
        return;
    };
    let custom = load_custom_games();
    if let Some(g) = custom.iter().find(|g| g.name.to_lowercase() == game.to_lowercase())
        .or_else(|| custom.iter().find(|g| g.name.to_lowercase().contains(&game.to_lowercase()))) {
        match launch_custom_game_2d(g, &cfg) { Ok(m) => println!("{m}"), Err(e) => { eprintln!("{e}"); std::process::exit(1); } }
        return;
    }
    if let Ok(appid) = game.parse::<u64>() {
        match launch_steam_2d(appid, &backend) { Ok(m) => println!("{m}"), Err(e) => { eprintln!("{e}"); std::process::exit(1); } }
        return;
    }
    if Path::new(&game).exists() {
        let g = CustomGame { name: Path::new(&game).file_name().unwrap_or_default().to_string_lossy().to_string(), path: PathBuf::from(&game), backend: backend.clone(), ..Default::default() };
        match launch_custom_game_2d(&g, &cfg) { Ok(m) => println!("{m}"), Err(e) => { eprintln!("{e}"); std::process::exit(1); } }
        return;
    }
    let games = list_games(&cfg);
    if let Some(g) = games.iter().find(|g| g.name.to_lowercase().contains(&game.to_lowercase())) {
        match launch_steam_2d(g.appid, &backend) { Ok(m) => println!("{m}"), Err(e) => { eprintln!("{e}"); std::process::exit(1); } }
    } else {
        eprintln!("Could not resolve game '{game}' for 2D mode. Try `vrdoctor list-games`.");
        std::process::exit(2);
    }
}

fn install_cli(args: &[String]) {
    let detected = current_distro();
    let family = match flag_value(args, "--distro").as_deref() {
        Some("arch") => Family::Arch,
        Some("fedora") => Family::Fedora,
        _ => detected.family.clone(),
    };
    let apply = args.iter().any(|a| a == "--apply");
    println!("VRDoctor installer — installing for {:?}{}\n", family, if apply { "" } else { " (dry run)" });
    for step in install_plan(&family) { println!("  {step}"); }
    println!("\nOn the Quest 3 itself (one-time):");
    for step in quest_setup_steps() { println!("  {step}"); }
    if !apply { println!("\nDry run only. Re-run with --apply, or use the GUI installer: vrdoctor installer"); return; }
    for step in install_plan(&family) {
        let cmd = step.trim();
        if cmd.is_empty() || cmd.starts_with('#') { continue; }
        println!("\n$ {cmd}");
        match Command::new("sh").arg("-c").arg(cmd).status() {
            Ok(s) if s.success() => println!("  ok"),
            Ok(s) => println!("  exited with {s} — continuing"),
            Err(e) => println!("  failed: {e}"),
        }
    }
    // If the user runs Flatpak Steam, make sure its sandbox can see WiVRn.
    if detect_steam_flavour(&load_config()) == SteamFlavour::Flatpak {
        match fix_flatpak_steam_visibility() { Ok(m) => println!("\nFlatpak Steam ↔ WiVRn:\n{m}"), Err(e) => println!("\nFlatpak Steam ↔ WiVRn: {e}") }
    }
    // Register the in-headset VR UI so WiVRn lists "VRDoctor VR".
    match register_vr_desktop() { Ok(p) => println!("\nWiVRn launcher entry: {}", p.display()), Err(e) => println!("\nWiVRn launcher entry: could not write ({e})") }
    let _ = mark_installed();
    let _ = save_default_config();
    println!("\nMarked as installed. Run: vrdoctor doctor");
}

// ------------------------------------------------------- in-headset VR UI

/// ~/.local/share/applications (or $XDG_DATA_HOME/applications).
fn applications_dir() -> PathBuf {
    env::var_os("XDG_DATA_HOME").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".local/share"))
        .join("applications")
}

/// The vrdoctor-vr binary sitting next to this executable, if present.
pub(crate) fn vr_binary_path() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let cand = exe.with_file_name("vrdoctor-vr");
    if cand.is_file() { Some(cand) } else { None }
}

/// The .desktop entry WiVRn's in-headset launcher picks up: any desktop
/// file whose Categories contain X-WiVRn-VR is listed as a VR app.
fn vr_desktop_entry(exec: &str) -> String {
    format!("[Desktop Entry]\nType=Application\nName=VRDoctor VR\nComment=Your VRDoctor library in VR — pick a game with your controllers\nExec={exec}\nIcon=vrdoctor\nTerminal=false\nCategories=Game;X-WiVRn-VR;\n")
}

/// Write/refresh ~/.local/share/applications/vrdoctor-vr.desktop so
/// WiVRn offers "VRDoctor VR" in its launcher. Inside an AppImage the
/// entry points back at the AppImage (`…AppImage vr`), because the
/// mounted binary path is temporary; a system install points straight
/// at the vrdoctor-vr binary.
pub(crate) fn register_vr_desktop() -> std::io::Result<PathBuf> {
    let exec = match env::var("APPIMAGE") {
        Ok(p) if !p.is_empty() => format!("\"{p}\" vr"),
        _ => match vr_binary_path() {
            Some(p) => format!("\"{}\"", p.display()),
            None => "vrdoctor-vr".to_string(),
        },
    };
    let dir = applications_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join("vrdoctor-vr.desktop");
    fs::write(&path, vr_desktop_entry(&exec))?;
    Ok(path)
}

fn cmd_vr(args: &[String]) {
    let register_only = args.iter().any(|a| a == "--register-only");
    match register_vr_desktop() {
        Ok(p) => println!("WiVRn launcher entry written: {}", p.display()),
        Err(e) => {
            eprintln!("Could not write the WiVRn launcher entry: {e}");
            if register_only { std::process::exit(1); }
        }
    }
    if register_only { return; }
    let Some(bin) = vr_binary_path() else {
        eprintln!("vrdoctor-vr was not found next to this executable. It ships with VRDoctor — reinstall, or use the AppImage (which bundles it).");
        std::process::exit(1);
    };
    // Forward extra args (e.g. --probe, --frames N) to the VR binary.
    let forward: Vec<&String> = args.iter().filter(|a| *a != "--register-only").collect();
    use std::os::unix::process::CommandExt;
    let err = Command::new(&bin).args(&forward).exec();
    eprintln!("Failed to start vrdoctor-vr: {err}");
    std::process::exit(1);
}

fn remove_game_cli(args: &[String]) {
    let name = flag_value(args, "--name").or_else(|| args.iter().find(|a| !a.starts_with('-')).cloned());
    let Some(name) = name else { eprintln!("remove-game needs --name \"My Game\""); std::process::exit(2); };
    match remove_custom_game(&name) {
        Ok(true) => println!("Removed custom game '{name}'."),
        Ok(false) => { eprintln!("No custom game named '{name}'."); std::process::exit(1); }
        Err(e) => { eprintln!("Failed: {e}"); std::process::exit(1); }
    }
}

// ---------------------------------------------------------------- main

fn usage() {
    println!(r#"VRDoctor — PCVR games on Meta Quest 3 from Arch / Fedora Linux

USAGE:
  vrdoctor                          Open the GUI (dark mode by default); on first run, opens the installer
  vrdoctor gui                      Open the GUI
  vrdoctor installer                Open the installer (asks Arch or Fedora, installs accordingly)
  vrdoctor install [--distro arch|fedora] [--apply]   CLI installer for the chosen distro
  vrdoctor doctor                   Check distro, GPU, Steam, backends, headset, 2D overlay
  vrdoctor setup [--apply]          Same plan as install, for your detected distro
  vrdoctor home [--theme dawn|meadow|ocean|sunset|night|studio|cozy] [--brightness 1.10]
                                    Generate + install a VR Home environment (360° panorama, no SteamVR needed)
  vrdoctor bar                      Open the Quest-style bar (bottom of desktop / under the screen in VR)
  vrdoctor play2d [--game <custom-name|appid|steam-name|path>]   Play a 2D game on the big screen in VR
  vrdoctor devices                  Detect Quest 3 over USB (adb)
  vrdoctor fix-openvr [--undo]      Stop a (broken) SteamVR from hijacking OpenVR games — route them to WiVRn/xrizer
  vrdoctor fix-steam              Let Flatpak Steam see WiVRn (sandbox overrides + runtime symlink) so games open in the headset, not flat on the monitor
  vrdoctor uninstall [--yes]      Remove VRDoctor's own files (settings, caches, menu entries); without --yes it only lists what would go
  vrdoctor update [--check]       Update the AppImage in place from GitHub releases (--check only reports)
  vrdoctor list-games [--online]    Steam games grouped VR / VR+2D / 2D + custom (non-store) games
  vrdoctor list-games --json        The same list as JSON (used by vrdoctor-vr, the in-headset UI)
  vrdoctor vr [--register-only]     Open the VRDoctor library inside the headset (a real OpenXR app
                                    that WiVRn lists under "VRDoctor VR"); --register-only only
                                    refreshes its WiVRn launcher entry (~/.local/share/applications)
  vrdoctor classify --game <appid|name> [--offline]   Is this Steam game VR or 2D? (auto-detection)
  vrdoctor add-game --name "My Game" --path /path/to/game [--args "..."] [--workdir /dir] [--backend wivrn|alvr|steamlink] [--proton /path/to/proton] [--mode vr|2d]
  vrdoctor remove-game --name "My Game"
  vrdoctor launch [--backend wivrn|alvr|steamlink] [--game <custom-name|appid|steam-name|path>]
  vrdoctor config                   Create/show config (~/.config/vrdoctor/config.conf)

Custom games: almost any game file — executables (ELF/shebang/+x, any extension),
.AppImage, scripts (.sh .py .pl .rb .js), .jar, .love, Windows .exe/.bat/.cmd/.msi
(via Proton, auto-detected from Steam, or Wine), .desktop entries, Quest .apk
(sideloaded to the headset), .flatpak/.flatpakref, archives (.zip .tar.gz .7z .rar …,
extracted to ~/.local/share/vrdoctor/library), and folders containing a game.
Add them in the GUI or with add-game. Stored in ~/.config/vrdoctor/custom_games.conf.

Backends: wivrn (recommended, Linux-native) · alvr (SteamVR fallback) · steamlink
GUI theme defaults to dark (theme=dark in config; toggle inside the GUI).
"#);
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(cmd) = args.first().map(|s| s.as_str()) else {
        // Desktop / bare launch: GUI when there's a display, help otherwise.
        // First run (e.g. from the AppImage) opens the installer first.
        if env::var_os("DISPLAY").is_some() || env::var_os("WAYLAND_DISPLAY").is_some() {
            if !install_done() {
                if let Err(e) = gui::run_installer() { eprintln!("Installer failed: {e}"); }
                if install_done() { if let Err(e) = gui::run_gui() { eprintln!("GUI failed: {e}"); } }
            } else if let Err(e) = gui::run_gui() { eprintln!("GUI failed: {e}"); usage(); }
        } else { usage(); }
        return;
    };
    match cmd {
        "gui" => { if let Err(e) = gui::run_gui() { eprintln!("GUI failed: {e}"); std::process::exit(1); } }
        "installer" => { if let Err(e) = gui::run_installer() { eprintln!("Installer failed: {e}"); std::process::exit(1); } }
        "install" => install_cli(&args[1..]),
        "bar" => { if let Err(e) = gui::run_bar() { eprintln!("Bar failed: {e}"); std::process::exit(1); } }
        "home" => {
            let brightness = flag_value(&args[1..], "--brightness").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.10);
            let theme = flag_value(&args[1..], "--theme").unwrap_or_else(|| "dawn".to_string());
            match home_install(&theme, brightness) { Ok(m) => { println!("{m}"); let mut c = load_config(); c.home_theme = theme.clone(); c.home_brightness = brightness; let _ = save_config(&c); }, Err(e) => { eprintln!("{e}"); std::process::exit(1); } }
        }
        "play2d" => play2d(&args[1..]),
        "doctor" => std::process::exit(doctor()),
        "setup" => do_setup(args.iter().any(|a| a == "--apply")),
        "devices" => {
            let devs = detect_quest();
            if devs.is_empty() { println!("No adb devices found. Wireless WiVRn does not need USB — check `vrdoctor doctor`."); }
            for (d, m, dev) in devs { println!("{}  {}  model={} device={} {}", d.serial, d.state, m.clone().unwrap_or_default(), dev.clone().unwrap_or_default(), if is_quest3(&m, &dev) { "← Quest 3" } else { "" }); }
        }
        "list-games" | "games" => {
            let cfg = load_config();
            if args.iter().any(|a| a == "--json") {
                println!("{}", games_json(&list_games(&cfg), &load_custom_games()));
                return;
            }
            let online = args.iter().any(|a| a == "--online");
            let games = list_games(&cfg);
            println!("Steam games{}:", if online { " (checking Steam online for unknowns)" } else { "" });
            if games.is_empty() { println!("  (none found — is Steam installed with games in ~/.local/share/Steam?)"); }
            for sup in [VrSupport::VrOnly, VrSupport::VrSupported, VrSupport::Flat, VrSupport::Unknown] {
                let group: Vec<&Game> = games.iter().filter(|g| classify_game(g, online).0 == sup).collect();
                if group.is_empty() { continue; }
                let title = match sup { VrSupport::VrOnly => "🥽 VR games", VrSupport::VrSupported => "🥽🖥 VR + 2D games", VrSupport::Flat => "🖥 2D games", VrSupport::Unknown => "❓ Not checked yet (run: vrdoctor list-games --online)" };
                println!(" {title}:");
                for g in group {
                    let (_, src) = classify_game(g, false);
                    println!("   [{}] {:>10}  {}  ({})", sup.label(), g.appid, g.name, src);
                }
            }
            let custom = load_custom_games();
            println!("\nCustom games (not from a store):");
            if custom.is_empty() { println!("  (none yet — add one with `vrdoctor add-game` or in the GUI)"); }
            for g in custom { println!("  [custom{}]  {}  →  {}{}", if g.mode == "2d" { " · 2D" } else { "" }, g.name, g.path.display(), if g.args.is_empty() { String::new() } else { format!("  {}", g.args) }); }
        }
        "classify" => {
            let online = !args.iter().any(|a| a == "--offline");
            let target = flag_value(&args[1..], "--game").or_else(|| args[1..].iter().find(|a| !a.starts_with('-')).cloned());
            let Some(target) = target else { eprintln!("classify needs --game <appid|name>"); std::process::exit(2); };
            if let Ok(appid) = target.parse::<u64>() {
                let (sup, src) = classify_appid(appid, online);
                println!("{appid}: {} ({})", sup.label(), src);
            } else {
                let cfg = load_config();
                if let Some(g) = list_games(&cfg).into_iter().find(|g| g.name.to_lowercase().contains(&target.to_lowercase())) {
                    let (sup, src) = classify_game(&g, online);
                    println!("{} ({}): {} ({})", g.name, g.appid, sup.label(), src);
                } else { eprintln!("No installed Steam game matching '{target}'"); std::process::exit(1); }
            }
        }
        "fix-openvr" => {
            let res = if args.iter().any(|a| a == "--undo") { fix_openvr_undo() } else { fix_openvr() };
            match res { Ok(m) => println!("{m}"), Err(e) => { eprintln!("{e}"); std::process::exit(1); } }
        }
        "fix-steam" => {
            match fix_flatpak_steam_visibility() { Ok(m) => println!("{m}"), Err(e) => { eprintln!("{e}"); std::process::exit(1); } }
        }
        "vr" => cmd_vr(&args[1..]),
        "uninstall" => cmd_uninstall(&args[1..]),
        "update" => cmd_update(&args[1..]),
        "add-game" => add_game_cli(&args[1..]),
        "remove-game" => remove_game_cli(&args[1..]),
        "launch" | "play" => launch(&args[1..]),
        "config" => {
            match save_default_config() { Ok(p) => { println!("Config: {}", p.display()); if let Ok(t) = fs::read_to_string(&p) { print!("{t}"); } println!("Custom games file: {}", custom_games_path().display()); } Err(e) => eprintln!("{e}") }
        }
        "-h" | "--help" | "help" => usage(),
        other => { eprintln!("Unknown command '{other}'\n"); usage(); std::process::exit(2); }
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique temp path per call — this environment's test harness can
    /// register a test twice, and two copies running at once must not
    /// share a PID-keyed fixture directory.
    fn unique_temp(prefix: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        env::temp_dir().join(format!("{prefix}-{}-{n}", std::process::id()))
    }
    #[test]
    fn detects_arch() {
        assert_eq!(parse_os_release("ID=arch\nPRETTY_NAME=\"Arch Linux\"\n").family, Family::Arch);
    }
    #[test]
    fn detects_fedora_via_id_like() {
        assert_eq!(parse_os_release("ID=nobara\nID_LIKE=\"fedora\"\n").family, Family::Fedora);
    }
    #[test]
    fn parses_manifest() {
        let m = "\"AppState\"\n{\n\t\"appid\"\t\t\"546560\"\n\t\"name\"\t\t\"Half-Life: Alyx\"\n}\n";
        assert_eq!(parse_appmanifest(m), Some((546560, "Half-Life: Alyx".to_string())));
    }
    #[test]
    fn parses_adb() {
        let devs = parse_adb_devices("List of devices attached\n2G0YC5ZG9H0123\tdevice\nemulator-5554\toffline\n");
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].state, "device");
    }
    #[test]
    fn quest3_detection() {
        assert!(is_quest3(&Some("Quest 3".into()), &None));
        assert!(is_quest3(&None, &Some("eureka".into())));
        assert!(!is_quest3(&Some("Quest 2".into()), &Some("hollywood".into())));
    }
    #[test]
    fn parses_custom_games() {
        let text = "# comment\nname=My Indie Game\npath=/games/indie/run.sh\nargs=--fullscreen\n\nname=Old Windows Game\npath=/games/old/game.exe\nbackend=alvr\n";
        let games = parse_custom_games(text);
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].name, "My Indie Game");
        assert_eq!(games[0].args, "--fullscreen");
        assert_eq!(games[1].backend, "alvr");
        assert!(is_windows_binary(&games[1].path));
        assert!(!is_windows_binary(&games[0].path));
    }
    #[test]
    fn splits_args_with_quotes() {
        assert_eq!(split_args(r#"--name "My Game" --path '/a b' plain"#), vec!["--name", "My Game", "--path", "/a b", "plain"]);
        assert_eq!(split_args(""), Vec::<String>::new());
    }
    #[test]
    fn default_theme_is_dark() {
        assert_eq!(Config::default().theme, "dark");
    }
    #[test]
    fn detects_file_kinds() {
        assert_eq!(detect_kind(Path::new("game.exe")), GameKind::WindowsExe);
        assert_eq!(detect_kind(Path::new("game.AppImage")), GameKind::AppImage);
        assert_eq!(detect_kind(Path::new("game.py")), GameKind::Python);
        assert_eq!(detect_kind(Path::new("game.jar")), GameKind::Jar);
        assert_eq!(detect_kind(Path::new("game.apk")), GameKind::AndroidApk);
        assert_eq!(detect_kind(Path::new("game.desktop")), GameKind::DesktopEntry);
        assert_eq!(detect_kind(Path::new("game.zip")), GameKind::Archive);
        assert_eq!(detect_kind(Path::new("game.tar.gz")), GameKind::Archive);
        assert_eq!(detect_kind(Path::new("game.7z")), GameKind::Archive);
        assert_eq!(detect_kind(Path::new("game.flatpakref")), GameKind::FlatpakRef);
        assert_eq!(detect_kind(Path::new("game.msi")), GameKind::WindowsInstaller);
        assert_eq!(detect_kind(Path::new("notes.txt")), GameKind::Unknown);
    }
    #[test]
    fn parses_desktop_exec() {
        let d = "[Desktop Entry]\nName=X\nExec=env FOO=1 /games/run --opt %U\n";
        // env is kept as the program token; field code %U is dropped.
        let (prog, args) = parse_desktop_exec(d).unwrap();
        assert_eq!(prog, "env");
        assert_eq!(args, vec!["FOO=1", "/games/run", "--opt"]);
        let d2 = "Exec=\"/my games/run.sh\" %f\n";
        let (prog2, args2) = parse_desktop_exec(d2).unwrap();
        assert_eq!(prog2, "/my games/run.sh");
        assert!(args2.is_empty());
    }
    #[test]
    fn finds_executable_in_folder() {
        let dir = unique_temp("vrdoctor-test");
        let sub = dir.join("game");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("readme.txt"), "hi").unwrap();
        fs::write(sub.join("game.exe"), "fake").unwrap();
        let native = sub.join("run.sh");
        fs::write(&native, "#!/bin/sh\n").unwrap();
        // Native script outranks the .exe.
        assert_eq!(find_executable_in(&dir), Some(native));
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn setup_plans_differ_per_distro() {
        let arch = setup_plan_for_family(&Family::Arch).join("\n");
        assert!(arch.contains("pacman"));
        assert!(arch.contains("wivrn-dashboard"));
        let fedora = setup_plan_for_family(&Family::Fedora).join("\n");
        assert!(fedora.contains("dnf install"));
        assert!(fedora.contains("wivrn"));
        assert!(!fedora.contains("pacman"));
    }
    #[test]
    fn vrsettings_background_insert_and_replace() {
        let doc = "{\n\t\"steamvr\" : {\n\t\t\"enableHomeApp\" : true\n\t}\n}";
        let out = set_vrsettings_background(doc, "/home/x/home.png");
        assert!(out.contains("\"background\" : \"/home/x/home.png\""));
        let out2 = set_vrsettings_background(&out, "/home/x/brighter.png");
        assert!(out2.contains("brighter.png"));
        assert!(!out2.contains("home.png\""));
        assert_eq!(out2.matches("\"background\"").count(), 1);
    }
    #[test]
    fn panorama_renders_png() {
        let p = unique_temp("vrdoctor-pano").with_extension("png");
        render_panorama(&p, 1.10).unwrap();
        let data = fs::read(&p).unwrap();
        assert_eq!(&data[..8], b"\x89PNG\r\n\x1a\n");
        assert!(data.len() > 20_000, "panorama should be a real image, got {} bytes", data.len());
        let _ = fs::remove_file(&p);
    }
    #[test]
    #[test]
    fn vrpath_parsing_and_building() {
        let content = "{\n\t\"runtime\" : [\n\t\t\"/home/u/.local/share/Steam/steamapps/common/SteamVR\"\n\t],\n\t\"version\" : 1\n}";
        let r = vrpath_runtimes(content);
        assert_eq!(r.len(), 1);
        assert!(r[0].contains("SteamVR"));
        let built = build_vrpath(Path::new("/usr/share/xrizer"));
        assert!(vrpath_runtimes(&built)[0].contains("xrizer"));
        assert!(vrpath_runtimes("{}").is_empty());
        assert!(vrpath_runtimes("no runtime key").is_empty());
    }

    #[test]
    fn store_categories_classification() {
        assert_eq!(parse_store_categories(r#"{"450390":{"success":true,"data":{"categories":[{"id":53,"description":"VR Only"}]}}}"#), Some(VrSupport::VrOnly));
        assert_eq!(parse_store_categories(r#"{"438100":{"success":true,"data":{"categories":[{"id":54,"description":"VR Supported"}]}}}"#), Some(VrSupport::VrSupported));
        assert_eq!(parse_store_categories(r#"{"570":{"success":true,"data":{"categories":[{"id":2,"description":"Single-player"}]}}}"#), Some(VrSupport::Flat));
        assert_eq!(parse_store_categories(r#"{"999":{"success":false}}"#), None);
        assert_eq!(parse_store_categories(r#"{"1":{"success":true,"data":{"genres":[{"description":"VR Support"}]}}}"#), Some(VrSupport::Flat));
    }

    #[test]
    fn known_titles_classify_offline() {
        let g = |appid| Game { appid, name: String::new(), library: PathBuf::new(), install_dir: PathBuf::new() };
        assert_eq!(classify_offline(&g(450390)).0, VrSupport::VrOnly);
        assert_eq!(classify_offline(&g(546560)).0, VrSupport::VrOnly);
        assert_eq!(classify_offline(&g(438100)).0, VrSupport::VrSupported);
        assert_eq!(classify_offline(&g(223750)).0, VrSupport::VrSupported);
        assert_eq!(classify_offline(&g(999999999)).0, VrSupport::Unknown);
    }

    #[test]
    fn manifest_installdir_parsed() {
        let m = "\"AppState\"\n{\n\t\"appid\"\t\t\"450390\"\n\t\"name\"\t\t\"The Lab\"\n\t\"installdir\"\t\t\"The Lab\"\n}";
        let (appid, name, dir) = parse_appmanifest_full(m).unwrap();
        assert_eq!(appid, 450390);
        assert_eq!(name, "The Lab");
        assert_eq!(dir, "The Lab");
    }

    #[test]
    fn all_home_themes_render() {
        for theme in HOME_THEMES {
            let p = std::env::temp_dir().join(format!("vrd-home-{theme}.png"));
            render_panorama_theme(&p, theme, 1.10).unwrap();
            let meta = fs::metadata(&p).unwrap();
            assert!(meta.len() > 10_000, "theme {theme} panorama suspiciously small");
        }
        assert!(home_install("bogus-theme", 1.10).is_err());
    }

    #[test]
    #[test]
    fn art_candidates_and_paths() {
        let c = art_candidates(450390);
        assert_eq!(c.len(), 3);
        assert!(c[0].ends_with("/450390/library_600x900.jpg"));
        assert!(c[1].ends_with("/450390/capsule_616x353.jpg"));
        assert!(art_path(450390).ends_with("vrdoctor/art/450390.jpg"));
    }

    #[test]
    fn elf_magic_counts_as_native() {
        let p = unique_temp("vrdoctor-elf");
        fs::write(&p, [0x7f, b'E', b'L', b'F', 2, 1]).unwrap();
        assert_eq!(detect_kind(&p), GameKind::NativeExecutable);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn games_json_shape_and_escaping() {
        // 450390 (The Lab) is a known VR-only title; 999999999 is unknown
        // offline (its install dir does not exist, so no VR markers).
        let steam = vec![
            Game { appid: 450390, name: "The \"Lab\" \\ VR".into(), library: PathBuf::new(), install_dir: PathBuf::new() },
            Game { appid: 999999999, name: "Mystery Game".into(), library: PathBuf::new(), install_dir: PathBuf::new() },
        ];
        let custom = vec![
            CustomGame { name: "Flat Friend".into(), mode: "2d".into(), ..Default::default() },
            CustomGame { name: "Homebrew VR".into(), mode: String::new(), ..Default::default() },
        ];
        let json = games_json(&steam, &custom);
        assert_eq!(json, concat!(
            "[",
            "{\"kind\":\"steam\",\"appid\":450390,\"name\":\"The \\\"Lab\\\" \\\\ VR\",\"vr\":\"vr\"},",
            "{\"kind\":\"steam\",\"appid\":999999999,\"name\":\"Mystery Game\",\"vr\":\"unknown\"},",
            "{\"kind\":\"custom\",\"appid\":0,\"name\":\"Flat Friend\",\"vr\":\"2d\"},",
            "{\"kind\":\"custom\",\"appid\":0,\"name\":\"Homebrew VR\",\"vr\":\"vr\"}",
            "]"
        ));
        assert_eq!(games_json(&[], &[]), "[]");
    }

    #[test]
    fn vr_desktop_entry_has_wivrn_category() {
        let entry = vr_desktop_entry("\"/opt/VRDoctor.AppImage\" vr");
        assert!(entry.contains("Name=VRDoctor VR"));
        assert!(entry.contains("Categories=Game;X-WiVRn-VR;"));
        assert!(entry.contains("Exec=\"/opt/VRDoctor.AppImage\" vr"));
    }

    #[test]

    #[test]

    #[test]
    fn version_parsing_and_ordering() {
        assert_eq!(parse_version("0.9.1"), Some((0, 9, 1)));
        assert_eq!(parse_version("v0.10.0"), Some((0, 10, 0)));
        assert_eq!(parse_version("1.0"), None);
        assert!(parse_version("0.10.0") > parse_version("0.9.1"));
        assert!(parse_version("0.9.1") > parse_version("0.9.0"));
    }

    #[test]
    fn release_payload_parsing() {
        let json = r#"{"tag_name":"v0.10.0","assets":[
            {"name":"notes.txt","browser_download_url":"https://example.com/notes"},
            {"name":"VRDoctor-x86_64.AppImage","browser_download_url":"https://example.com/VRDoctor-x86_64.AppImage"}
        ]}"#;
        let info = release_update(json).unwrap();
        assert_eq!(info.version, "0.10.0");
        assert_eq!(info.url, "https://example.com/VRDoctor-x86_64.AppImage");
        assert!(release_update("{}").is_none());
    }

    #[test]
    fn uninstall_removes_all_user_files() {
        let base = unique_temp("vrdoctor-uninstall-test");
        let _ = fs::remove_dir_all(&base);
        let config = base.join("config/vrdoctor");
        let legacy = base.join("config/quest3-launcher");
        let data = base.join("data");
        let cache = base.join("cache");
        for d in [&config, &legacy, &data.join("applications"), &data.join("vrdoctor/library"), &cache.join("vrdoctor/art")] { fs::create_dir_all(d).unwrap(); }
        fs::write(config.join("config.conf"), "backend=wivrn\n").unwrap();
        fs::write(data.join("applications/vrdoctor-vr.desktop"), "[Desktop Entry]\n").unwrap();
        fs::write(cache.join("vrdoctor/art/730.jpg"), "x").unwrap();
        let report = uninstall_at(&config, &legacy, &data, &cache);
        assert!(!config.exists() && !legacy.exists() && !data.join("vrdoctor").exists() && !cache.join("vrdoctor").exists());
        assert!(!data.join("applications/vrdoctor-vr.desktop").exists());
        assert!(report.iter().filter(|l| l.starts_with("Removed")).count() >= 4, "{report:?}");
        let again = uninstall_at(&config, &legacy, &data, &cache);
        assert!(again[0].starts_with("Nothing to remove"));
        let _ = fs::remove_dir_all(&base);
    }

    fn steam_flavour_prefers_library_owner() {
        // The user's exact case: Flatpak Steam has the games, native was
        // freshly installed by VRDoctor and has none → Flatpak wins.
        assert_eq!(pick_steam_flavour(true, true, 20, 0), SteamFlavour::Flatpak);
        assert_eq!(pick_steam_flavour(false, true, 0, 5), SteamFlavour::Native);
        assert_eq!(pick_steam_flavour(true, false, 0, 0), SteamFlavour::Flatpak);
        assert_eq!(pick_steam_flavour(false, true, 0, 0), SteamFlavour::Native);
        assert_eq!(pick_steam_flavour(false, false, 0, 0), SteamFlavour::None);
        // More games wins when both libraries are populated.
        assert_eq!(pick_steam_flavour(true, true, 3, 9), SteamFlavour::Native);
        assert_eq!(pick_steam_flavour(true, true, 9, 3), SteamFlavour::Flatpak);
    }

    #[test]
    fn steam_launch_command_uses_flatpak_run() {
        let flat = format!("{:?}", steam_launch_command(SteamFlavour::Flatpak, 275850));
        assert!(flat.contains("flatpak") && flat.contains("com.valvesoftware.Steam") && flat.contains("275850"), "{flat}");
        let nat = format!("{:?}", steam_launch_command(SteamFlavour::Native, 275850));
        assert!(nat.contains("\"steam\"") && nat.contains("275850"), "{nat}");
    }
}
