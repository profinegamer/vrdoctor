//! VRDoctor GUI (egui/eframe). Dark mode is the default — it is applied on
//! startup from config (`theme=dark` unless the user saved `theme=light`),
//! and the toggle in the header persists the choice back to config.

use eframe::egui;
use std::path::PathBuf;

use crate::{
    capture, classify_game, current_distro, fix_openvr, home_install, install_done, launch_steam_2d, list_games,
    load_config, load_custom_games, mark_installed, quest_setup_steps, register_custom_game, remove_custom_game,
    run_checks, run_installer_terminal, save_config, setup_plan, setup_plan_for_family, start_backend, start_bar,
    start_overlay, launch_custom_game, launch_steam_appid, CheckResult, Config, CustomGame, Family, Game, VrSupport,
    HOME_THEMES, SUPPORTED_KINDS,
};

#[derive(PartialEq, Clone, Copy)]
enum Tab { Games, Doctor, AddGame, Shell, Setup }

pub struct VrDoctorApp {
    cfg: Config,
    dark_mode: bool,
    tab: Tab,
    backend: String,
    status: String,
    checks: Vec<CheckResult>,
    steam_games: Vec<Game>,
    custom_games: Vec<CustomGame>,
    search: String,
    // Add-game form
    f_name: String,
    f_path: String,
    f_args: String,
    f_workdir: String,
    f_backend: String,
    f_proton: String,
    f_mode: String,
    brightness: f32,
    home_theme: String,
}

impl VrDoctorApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let cfg = load_config();
        let dark_mode = cfg.theme != "light"; // dark is the default
        cc.egui_ctx.set_visuals(if dark_mode { egui::Visuals::dark() } else { egui::Visuals::light() });
        let backend = cfg.backend.clone();
        Self {
            steam_games: list_games(&cfg),
            custom_games: load_custom_games(),
            cfg,
            dark_mode,
            tab: Tab::Games,
            backend,
            status: "Ready. Pick a game, or run Doctor to check your setup.".into(),
            checks: vec![],
            search: String::new(),
            f_name: String::new(),
            f_path: String::new(),
            f_args: String::new(),
            f_workdir: String::new(),
            f_backend: String::new(),
            f_proton: String::new(),
            f_mode: "vr".to_string(),
            brightness: 1.10,
            home_theme: "dawn".into(),
        }
    }

    fn refresh_games(&mut self) {
        self.steam_games = list_games(&self.cfg);
        self.custom_games = load_custom_games();
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        ctx.set_visuals(if self.dark_mode { egui::Visuals::dark() } else { egui::Visuals::light() });
        self.cfg.theme = if self.dark_mode { "dark".into() } else { "light".into() };
        let _ = save_config(&self.cfg);
    }
}

impl eframe::App for VrDoctorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("🩺 VRDoctor");
                ui.label("PCVR on Meta Quest 3 — Arch & Fedora Linux");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.checkbox(&mut self.dark_mode, "🌙 Dark mode").changed() {
                        self.apply_theme(ctx);
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Backend:");
                for (b, label) in [("wivrn", "wivrn (no SteamVR)"), ("alvr", "alvr (needs SteamVR)"), ("steamlink", "steamlink (needs SteamVR)")] {
                    ui.selectable_value(&mut self.backend, b.to_string(), label);
                }
                if ui.button("▶ Start backend").clicked() {
                    self.status = start_backend(&self.backend.clone());
                }
                if ui.button("⟳ Refresh games").clicked() { self.refresh_games(); }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Games, "🎮 Games");
                ui.selectable_value(&mut self.tab, Tab::Doctor, "🩺 Doctor");
                ui.selectable_value(&mut self.tab, Tab::AddGame, "➕ Add custom game");
                ui.selectable_value(&mut self.tab, Tab::Shell, "🌐 VR Shell");
                ui.selectable_value(&mut self.tab, Tab::Setup, "⚙ Setup");
            });
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| { ui.label(&self.status); });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.tab {
                Tab::Games => self.games_tab(ui),
                Tab::Doctor => self.doctor_tab(ui),
                Tab::AddGame => self.add_game_tab(ui),
                Tab::Shell => self.shell_tab(ui),
                Tab::Setup => self.setup_tab(ui),
            }
        });
    }
}

impl VrDoctorApp {
    fn games_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut self.search);
        });
        let needle = self.search.to_lowercase();

        ui.heading("Custom games (not from a store)");
        if self.custom_games.is_empty() {
            ui.label("None yet — use the “Add custom game” tab to add any local game, store or not.");
        }
        let mut to_launch: Option<CustomGame> = None;
        let mut to_remove: Option<String> = None;
        for g in self.custom_games.iter().filter(|g| needle.is_empty() || g.name.to_lowercase().contains(&needle)) {
            ui.horizontal(|ui| {
                if ui.button("▶ Launch").clicked() { to_launch = Some(g.clone()); }
                ui.vertical(|ui| {
                    ui.strong(format!("{}{}", g.name, if g.mode == "2d" { "  [2D]" } else { "" }));
                    ui.small(format!("{}{}", g.path.display(), if g.args.is_empty() { String::new() } else { format!("  {}", g.args) }));
                });
                if ui.button("🗑").clicked() { to_remove = Some(g.name.clone()); }
            });
        }
        if let Some(g) = to_launch {
            let mut g = g;
            if g.backend.is_empty() { g.backend = self.backend.clone(); }
            self.status = match launch_custom_game(&g, &self.cfg) { Ok(m) => m, Err(e) => e };
        }
        if let Some(name) = to_remove {
            self.status = match remove_custom_game(&name) {
                Ok(true) => format!("Removed custom game '{name}'."),
                Ok(false) => format!("No custom game named '{name}'."),
                Err(e) => format!("Failed to remove: {e}"),
            };
            self.refresh_games();
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.heading("Steam games");
            if ui.button("🔍 Check Steam online (VR or 2D?)").clicked() {
                let mut known = 0;
                for g in &self.steam_games { if classify_game(g, true).0 != VrSupport::Unknown { known += 1; } }
                self.status = format!("Checked Steam: {known}/{} games classified. Badges: [VR] VR-only · [VR+2D] plays both ways · [2D] flat game · [?] not checked.", self.steam_games.len());
            }
        });
        ui.small("VRDoctor auto-detects whether each Steam game is VR or 2D. Launching a 2D game sends it to the big screen automatically.");
        if self.steam_games.is_empty() {
            ui.label("No Steam games found. Check Doctor — Steam may not be installed yet.");
        }
        let mut steam_launch: Option<Game> = None;
        let mut steam_launch_2d: Option<Game> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for g in self.steam_games.iter().filter(|g| needle.is_empty() || g.name.to_lowercase().contains(&needle)) {
                ui.horizontal(|ui| {
                    if ui.button("▶ Launch").clicked() { steam_launch = Some(g.clone()); }
                    if ui.button("▶ 2D").clicked() { steam_launch_2d = Some(g.clone()); }
                    let (sup, _src) = classify_game(g, false);
                    ui.strong(format!("[{}]", sup.label()));
                    ui.label(format!("{}  ({})", g.name, g.appid));
                });
            }
        });
        if let Some(g) = steam_launch {
            self.status = match launch_steam_appid(g.appid, &self.backend.clone()) { Ok(m) => m, Err(e) => e };
        }
        if let Some(g) = steam_launch_2d {
            self.status = match launch_steam_2d(g.appid, &self.backend.clone()) { Ok(m) => m, Err(e) => e };
        }
    }

    fn doctor_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Doctor — is this PC ready for Quest 3 PCVR?");
        if ui.button("🩺 Run checks").clicked() { self.checks = run_checks(); }
        if self.checks.is_empty() { ui.label("Press “Run checks”."); }
        for c in &self.checks {
            let (icon, color) = if c.ok { ("●", egui::Color32::from_rgb(80, 200, 120)) } else { ("●", egui::Color32::from_rgb(230, 90, 90)) };
            ui.horizontal(|ui| {
                ui.colored_label(color, icon);
                ui.strong(&c.label);
                ui.label(&c.detail);
            });
        }
        ui.horizontal(|ui| {
            if ui.button("🛠 Fix OpenVR routing (stop SteamVR hijacking games)").clicked() {
                self.status = match fix_openvr() { Ok(m) => m, Err(e) => e };
            }
        });
        ui.small("VRDoctor never needs SteamVR — WiVRn is a complete runtime on its own. The fix above only matters if a broken SteamVR install is redirecting your OpenVR games.");
        ui.separator();
        ui.label("Firewall reminder: allow UDP 5353 (mDNS) and TCP/UDP 9757 (WiVRn). ALVR uses TCP/UDP 9943–9944.");
    }

    fn add_game_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Add a custom game");
        ui.label("Any game that isn't on Steam or another store. Point it at the game file itself, an archive of the game, or the game's folder — VRDoctor works out the rest.");
        ui.label(format!("Supported: {SUPPORTED_KINDS}."));
        ui.small("Archives (.zip, .tar.gz, .7z…) are extracted to ~/.local/share/vrdoctor/library and the game inside is registered. A Quest .apk is sideloaded onto the headset. Windows files run via Proton (auto-detected) or Wine.");
        egui::Grid::new("add_game_form").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label("Name *");
            ui.text_edit_singleline(&mut self.f_name);
            ui.end_row();
            ui.label("Game file / archive / folder *");
            ui.text_edit_singleline(&mut self.f_path);
            ui.end_row();
            ui.label("Arguments");
            ui.text_edit_singleline(&mut self.f_args);
            ui.end_row();
            ui.label("Working directory");
            ui.text_edit_singleline(&mut self.f_workdir);
            ui.end_row();
            ui.label("Backend override");
            ui.text_edit_singleline(&mut self.f_backend);
            ui.end_row();
            ui.label("Proton path (.exe only)");
            ui.text_edit_singleline(&mut self.f_proton);
            ui.end_row();
            ui.label("Mode");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.f_mode, "vr".to_string(), "🥽 VR game");
                ui.selectable_value(&mut self.f_mode, "2d".to_string(), "🖥 2D (big screen in VR)");
            });
            ui.end_row();
        });
        ui.small("Backend override and Proton path can be left blank — VRDoctor will use your default backend and find Proton from Steam.");
        if ui.button("➕ Add game").clicked() {
            if self.f_name.trim().is_empty() || self.f_path.trim().is_empty() {
                self.status = "Name and executable path are required.".into();
            } else {
                let game = CustomGame {
                    name: self.f_name.trim().to_string(),
                    path: PathBuf::from(self.f_path.trim()),
                    args: self.f_args.trim().to_string(),
                    workdir: if self.f_workdir.trim().is_empty() { None } else { Some(PathBuf::from(self.f_workdir.trim())) },
                    backend: self.f_backend.trim().to_string(),
                    proton: self.f_proton.trim().to_string(),
                    source: String::new(),
                    mode: self.f_mode.clone(),
                };
                let name = game.name.clone();
                match register_custom_game(game) {
                    Ok((resolved, note, p)) => {
                        self.status = format!("Added custom game '{name}' → {} — saved in {}. {}", resolved.path.display(), p.display(), note);
                        self.f_name.clear(); self.f_path.clear(); self.f_args.clear();
                        self.f_workdir.clear(); self.f_backend.clear(); self.f_proton.clear();
                        self.refresh_games();
                        self.tab = Tab::Games;
                    }
                    Err(e) => self.status = format!("Failed to save game: {e}"),
                }
            }
        }
    }

    fn shell_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("VR Home — your environment in VR");
        ui.label("Pick an environment — not just darkness. Each theme is a 360° panorama VRDoctor renders for you; use it as the backdrop behind your desktop panel in VR. No SteamVR needed. Default brightness 110% — a touch brighter, not much.");
        ui.horizontal(|ui| {
            ui.label("Theme:");
            for t in HOME_THEMES {
                ui.selectable_value(&mut self.home_theme, t.to_string(), *t);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Brightness:");
            ui.add(egui::Slider::new(&mut self.brightness, 0.90..=1.30).text("×"));
            if ui.button("🏠 Install VR Home").clicked() {
                self.status = match home_install(&self.home_theme.clone(), self.brightness) { Ok(m) => m, Err(e) => e };
            }
        });
        ui.separator();
        ui.heading("2D games & the desktop panel");
        ui.label("2D games don't render in VR — they play on a big floating screen, like on the Steam Frame. VRDoctor does this with the desktop-in-VR overlay (WayVR / wlx-overlay-s):");
        ui.label("• With a 2D game running, the game fills the big screen.");
        ui.label("• With no 2D game, the same big panel shows your whole PC desktop.");
        ui.label("• The VRDoctor bar sits under the screen — Quest-style — with the time, backend status, Quest battery, and shortcuts. It lives at the bottom of your desktop, so it appears under the panel in VR.");
        ui.horizontal(|ui| {
            if ui.button("▶ Start 2D mode (backend + overlay + bar)").clicked() {
                self.status = format!("{}\n{}\n{}", start_backend(&self.backend.clone()), start_overlay(), start_bar());
            }
            if ui.button("▬ Start the bar only").clicked() { self.status = start_bar(); }
        });
        ui.small("Mark any game as 2D in the Add custom game tab (Mode: 2D), or launch a Steam game with the ▶ 2D button in the Games tab.");
    }

    fn setup_tab(&mut self, ui: &mut egui::Ui) {
        let distro = current_distro();
        ui.heading(format!("Setup — {}", distro.pretty));
        ui.horizontal(|ui| {
            ui.label(format!("Installer finished: {}", if install_done() { "yes" } else { "no" }));
            if ui.button("🧩 Open installer (choose Arch / Fedora)").clicked() {
                match std::env::current_exe() {
                    Ok(exe) => { let _ = std::process::Command::new(exe).arg("installer").spawn(); self.status = "Installer opened in a new window.".into(); }
                    Err(e) => self.status = format!("Could not open installer: {e}"),
                }
            }
        });
        ui.separator();
        ui.label("Run these in a terminal (or `vrdoctor setup --apply`):");
        egui::ScrollArea::vertical().show(ui, |ui| {
            for line in setup_plan(&distro) { ui.monospace(line); }
            ui.separator();
            ui.label("On the Quest 3 itself (one-time):");
            for line in quest_setup_steps() { ui.label(line); }
        });
    }
}

// ---------------------------------------------------------------- installer

/// First-run installer: asks which distro you're on — Arch or Fedora —
/// shows that distro's plan, and installs accordingly (in a terminal so
/// sudo can prompt). This is what the AppImage opens on first run.
pub struct InstallerApp {
    family: Family,
    detected: String,
    status: String,
}

impl InstallerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let cfg = load_config();
        let dark_mode = cfg.theme != "light";
        cc.egui_ctx.set_visuals(if dark_mode { egui::Visuals::dark() } else { egui::Visuals::light() });
        let distro = current_distro();
        let family = match distro.family { Family::Arch | Family::Fedora => distro.family.clone(), _ => Family::Arch };
        Self { family, detected: distro.pretty, status: "Pick your distro, review the plan, then install.".into() }
    }
}

impl eframe::App for InstallerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("🩺 VRDoctor installer");
            ui.label(format!("Detected: {}. If that's wrong, just pick the right one:", self.detected));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.family, Family::Arch, "🟦 Arch Linux");
                ui.selectable_value(&mut self.family, Family::Fedora, "🟥 Fedora");
            });
            ui.add_space(8.0);
            ui.label(format!("Install plan for {:?}:", self.family));
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for line in setup_plan_for_family(&self.family) { ui.monospace(line); }
            });
            ui.add_space(6.0);
            ui.label("Plus, on the Quest 3: install the WiVRn client app (version must match the PC server), and join the same 5/6 GHz Wi-Fi as the PC.");
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("⬇ Install for my distro").clicked() {
                    self.status = match run_installer_terminal(&self.family) { Ok(m) => m, Err(e) => e };
                }
                if ui.button("Skip — I'll set it up myself").clicked() {
                    let _ = mark_installed();
                    self.status = "Skipped. You can re-open this installer any time with `vrdoctor installer`.".into();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if ui.button("Open VRDoctor").clicked() {
                    let _ = mark_installed();
                    if let Ok(exe) = std::env::current_exe() { let _ = std::process::Command::new(exe).arg("gui").spawn(); }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.separator();
            ui.label(&self.status);
        });
    }
}

pub fn run_installer() -> Result<(), Box<dyn std::error::Error>> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("VRDoctor Installer — Arch or Fedora?")
            .with_inner_size([760.0, 560.0])
            .with_min_inner_size([640.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native("VRDoctor Installer", options, Box::new(|cc| Ok(Box::new(InstallerApp::new(cc)) as Box<dyn eframe::App>)))?;
    Ok(())
}

// ---------------------------------------------------------------- bar

/// The Quest-style bar: a slim, rounded, dark dock meant to sit at the
/// bottom of the desktop — so in VR it sits under the big screen, like
/// Quest's universal bar. Clock, backend status, Quest battery, shortcuts.
pub struct BarApp {
    battery: Option<String>,
    frames: u64,
}

impl BarApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark()); // the bar is always dark, like Quest's
        Self { battery: quest_battery(), frames: 0 }
    }
}

fn quest_battery() -> Option<String> {
    let out = capture("adb", &["shell", "dumpsys", "battery"])?;
    for line in out.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("level:") { return Some(format!("🔋 Quest {}%", v.trim())); }
    }
    None
}

impl eframe::App for BarApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frames += 1;
        if self.frames % 1800 == 0 { self.battery = quest_battery(); } // ~every 30 s at 60 fps
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
            let clock = capture("date", &["+%H:%M"]).map(|s| s.trim().to_string()).unwrap_or_default();
            let date = capture("date", &["+%a %d %b"]).map(|s| s.trim().to_string()).unwrap_or_default();
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(30, 32, 36))
                .rounding(egui::Rounding::same(28.0))
                .inner_margin(egui::Margin::symmetric(18.0, 8.0))
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.strong("🩺 VRDoctor");
                        ui.separator();
                        ui.label(format!("🕐 {clock}"));
                        ui.small(date);
                        ui.separator();
                        ui.label(if crate::which("wivrn-server").is_some() || crate::flatpak_installed("io.github.wivrn.wivrn") { "📶 WiVRn ready" } else { "📶 WiVRn not installed" });
                        if let Some(b) = &self.battery { ui.label(b.clone()); }
                        ui.separator();
                        if ui.button("🎮 Games").clicked() {
                            if let Ok(exe) = std::env::current_exe() { let _ = std::process::Command::new(exe).arg("gui").spawn(); }
                        }
                        if ui.button("🩺 Doctor").clicked() {
                            if let Ok(exe) = std::env::current_exe() { let _ = std::process::Command::new(exe).args(["gui"]).spawn(); }
                        }
                        if ui.button("🏠 Home").clicked() {
                            self.battery = Some(match home_install("dawn", 1.10) { Ok(_) => "Home installed ✓".into(), Err(e) => e });
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✕").clicked() { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
                        });
                    });
                });
        });
    }
}

pub fn run_bar() -> Result<(), Box<dyn std::error::Error>> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("VRDoctor Bar")
            .with_inner_size([880.0, 64.0])
            .with_min_inner_size([640.0, 56.0])
            .with_decorations(false)
            .with_always_on_top()
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native("VRDoctor Bar", options, Box::new(|cc| Ok(Box::new(BarApp::new(cc)) as Box<dyn eframe::App>)))?;
    Ok(())
}

pub fn run_gui() -> Result<(), Box<dyn std::error::Error>> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("VRDoctor — Quest 3 PCVR Launcher")
            .with_inner_size([1020.0, 660.0])
            .with_min_inner_size([760.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "VRDoctor",
        options,
        Box::new(|cc| Ok(Box::new(VrDoctorApp::new(cc)) as Box<dyn eframe::App>)),
    )?;
    Ok(())
}
