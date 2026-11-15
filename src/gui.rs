//! VRDoctor GUI (egui/eframe). Dark mode is the default — it is applied on
//! startup from config (`theme=dark` unless the user saved `theme=light`),
//! and the toggle in the header persists the choice back to config.

use eframe::egui;
use std::path::PathBuf;

use crate::{
    art_path, capture, classify_game, current_distro, download_art, fix_openvr, home_install, install_done, launch_steam_2d, list_games,
    load_config, load_custom_games, mark_installed, quest_setup_steps, register_custom_game, remove_custom_game,
    install_plan, register_vr_desktop, run_checks, uninstall_user_files, run_installer_terminal, save_config, setup_plan, setup_plan_for_family, start_backend, start_bar,
    start_overlay, launch_custom_game, launch_steam_appid, CheckResult, Config, CustomGame, Family, Game, VrSupport,
    HOME_THEMES, SUPPORTED_KINDS,
};

// ------------------------------------------------------------ visual theme

/// VRDoctor's polished theme: deep slate surfaces, a teal "medical" accent,
/// rounded corners and comfortable spacing — applied to the GUI, the
/// installer and the bar so everything looks like one product.
pub(crate) const ACCENT: egui::Color32 = egui::Color32::from_rgb(45, 212, 191);
/// The alternative accent (user-selectable in the header, persisted as
/// `accent=red` in config). The active accent lives in an atomic so the
/// theme helpers and all three apps share it without plumbing.
pub(crate) const ACCENT_RED: egui::Color32 = egui::Color32::from_rgb(239, 68, 68);
static ACCENT_PACKED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub(crate) fn accent() -> egui::Color32 {
    let p = ACCENT_PACKED.load(std::sync::atomic::Ordering::Relaxed);
    if p == 0 { ACCENT } else { egui::Color32::from_rgb((p >> 16) as u8, (p >> 8) as u8, p as u8) }
}
pub(crate) fn set_accent(c: egui::Color32) {
    ACCENT_PACKED.store(((c.r() as u32) << 16) | ((c.g() as u32) << 8) | c.b() as u32, std::sync::atomic::Ordering::Relaxed);
}
pub(crate) fn set_accent_from_config(cfg: &Config) { set_accent(if cfg.accent == "red" { ACCENT_RED } else { ACCENT }); }

fn polished_visuals(dark: bool) -> egui::Visuals {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let round = egui::Rounding::same(9.0);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open, &mut v.widgets.noninteractive] {
        w.rounding = round;
    }
    v.window_rounding = egui::Rounding::same(14.0);
    v.selection.bg_fill = accent();
    v.hyperlink_color = accent();
    if dark {
        v.panel_fill = egui::Color32::from_rgb(13, 18, 32);
        v.window_fill = egui::Color32::from_rgb(13, 18, 32);
        v.extreme_bg_color = egui::Color32::from_rgb(7, 10, 18);
        v.faint_bg_color = egui::Color32::from_rgb(24, 33, 52);
        v.override_text_color = Some(egui::Color32::from_rgb(226, 232, 240));
        v.widgets.inactive.bg_fill = egui::Color32::from_rgb(30, 41, 59);
        v.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(30, 41, 59);
        v.widgets.hovered.bg_fill = egui::Color32::from_rgb(43, 58, 84);
        v.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(43, 58, 84);
        v.widgets.active.bg_fill = egui::Color32::from_rgb(15, 118, 110);
        v.widgets.active.weak_bg_fill = egui::Color32::from_rgb(15, 118, 110);
        v.selection.stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(6, 24, 22));
    } else {
        v.panel_fill = egui::Color32::from_rgb(248, 250, 252);
        v.window_fill = egui::Color32::from_rgb(248, 250, 252);
        v.faint_bg_color = egui::Color32::from_rgb(226, 232, 240);
        v.widgets.active.bg_fill = egui::Color32::from_rgb(153, 246, 228);
        v.widgets.active.weak_bg_fill = egui::Color32::from_rgb(153, 246, 228);
    }
    v
}

fn apply_polish(ctx: &egui::Context, dark: bool) {
    ctx.set_visuals(polished_visuals(dark));
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(14.0);
    style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(23.0));
    style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
    style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
    style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(12.5));
    style.text_styles.insert(egui::TextStyle::Monospace, egui::FontId::monospace(13.5));
    ctx.set_style(style);
}

/// Coloured pill showing a Steam game's detected kind.
fn badge(ui: &mut egui::Ui, sup: VrSupport) {
    let (text, bg) = match sup {
        VrSupport::VrOnly => (" VR ", egui::Color32::from_rgb(21, 101, 52)),
        VrSupport::VrSupported => (" VR+2D ", egui::Color32::from_rgb(88, 28, 175)),
        VrSupport::Flat => (" 2D ", egui::Color32::from_rgb(29, 64, 175)),
        VrSupport::Unknown => (" ? ", egui::Color32::from_rgb(133, 77, 14)),
    };
    ui.label(egui::RichText::new(text).background_color(bg).color(egui::Color32::WHITE).strong().size(13.0));
}

/// A rounded card used for game rows and grouped content.
fn card<R>(ui: &mut egui::Ui, f: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let r = egui::Frame::none()
        .fill(ui.visuals().faint_bg_color)
        .rounding(egui::Rounding::same(10.0))
        .inner_margin(egui::Margin::symmetric(10.0, 7.0))
        .show(ui, f);
    ui.add_space(4.0);
    r.inner
}

#[derive(PartialEq, Clone, Copy)]
enum Tab { Games, Doctor, AddGame, Shell, Setup, Settings }

/// A launch requested by a tile, card or the detail bar — acted on after
/// the UI pass so borrows stay simple.
enum Pending { Steam(u64), Steam2d(u64), Custom(CustomGame) }


/// Background-download missing Steam cover art, then ask for a repaint.
fn spawn_art_download(ctx: &egui::Context, appids: Vec<u64>) {
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        for id in appids { if !art_path(id).exists() { download_art(id); } }
        ctx.request_repaint();
    });
}

/// Placeholder tile colour, picked from a muted jewel-tone palette.
fn tile_color(seed: u64) -> egui::Color32 {
    const PALETTE: [(u8, u8, u8); 8] = [
        (30, 64, 175), (21, 101, 52), (88, 28, 175), (154, 52, 18),
        (14, 116, 144), (134, 25, 143), (133, 77, 14), (51, 65, 85),
    ];
    let (r, g, b) = PALETTE[(seed % 8) as usize];
    egui::Color32::from_rgb(r, g, b)
}

fn truncate_name(name: &str, max: usize) -> String {
    if name.chars().count() <= max { return name.to_string(); }
    let mut out: String = name.chars().take(max - 1).collect();
    out.push('…');
    out
}

pub struct VrDoctorApp {
    cfg: Config,
    dark_mode: bool,
    accent_red: bool,
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
    // Library (Steam-style grid)
    grid_view: bool,
    selected_steam: Option<u64>,
    selected_custom: Option<String>,
    pending: Option<Pending>,
    textures: std::collections::HashMap<u64, egui::TextureHandle>,
    art_bad: std::collections::HashSet<u64>,
    /// VR/2D classification per Steam AppID, computed ONCE per refresh.
    /// classify_game() can recursively walk a game's install folder, so
    /// calling it per tile per frame (the v0.7.0 behaviour) made the GUI
    /// lag badly on real libraries.
    vr_info: std::collections::HashMap<u64, (VrSupport, String)>,
    confirm_uninstall: bool,
    set_theme: String,
    set_brightness: f32,
}

impl VrDoctorApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Keep the WiVRn launcher entry fresh on every start: upgrades
        // from a version without the VR UI (or a moved AppImage) would
        // otherwise leave WiVRn with no "VRDoctor VR" to list.
        let _ = register_vr_desktop();
        let cfg = load_config();
        let dark_mode = cfg.theme != "light"; // dark is the default
        set_accent_from_config(&cfg);
        apply_polish(&cc.egui_ctx, dark_mode);
        let backend = cfg.backend.clone();
        let games = list_games(&cfg);
        spawn_art_download(&cc.egui_ctx, games.iter().map(|g| g.appid).collect());
        let vr_info = games.iter().map(|g| (g.appid, classify_game(g, false))).collect();
        let accent_red = cfg.accent == "red";
        let set_theme = cfg.home_theme.clone();
        let set_brightness = cfg.home_brightness;
        Self {
            steam_games: games,
            custom_games: load_custom_games(),
            cfg,
            dark_mode,
            accent_red,
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
            grid_view: true,
            selected_steam: None,
            selected_custom: None,
            pending: None,
            textures: std::collections::HashMap::new(),
            art_bad: std::collections::HashSet::new(),
            vr_info,
            confirm_uninstall: false,
            set_theme,
            set_brightness,
        }
    }

    /// Load (once) and return the cover-art texture for a Steam game.
    fn texture_for(&mut self, ctx: &egui::Context, appid: u64) -> Option<egui::TextureHandle> {
        if let Some(t) = self.textures.get(&appid) { return Some(t.clone()); }
        if self.art_bad.contains(&appid) { return None; }
        let path = art_path(appid);
        if !path.exists() { return None; }
        match image::open(&path) {
            Ok(img) => {
                let img = if img.width() > 600 { img.resize(600, 900, image::imageops::FilterType::Triangle) } else { img };
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let ci = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                let tex = ctx.load_texture(format!("art-{appid}"), ci, egui::TextureOptions::LINEAR);
                self.textures.insert(appid, tex.clone());
                Some(tex)
            }
            Err(_) => { self.art_bad.insert(appid); None }
        }
    }

    fn refresh_games(&mut self) {
        self.steam_games = list_games(&self.cfg);
        self.custom_games = load_custom_games();
        self.reclassify();
    }

    /// Classify every Steam game once and cache the results for the UI.
    fn reclassify(&mut self) {
        self.vr_info = self.steam_games.iter().map(|g| (g.appid, classify_game(g, false))).collect();
    }

    /// Cached classification for a game (Unknown if not computed yet).
    fn class_of(&self, g: &Game) -> (VrSupport, String) {
        self.vr_info.get(&g.appid).cloned().unwrap_or((VrSupport::Unknown, "not checked".to_string()))
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        apply_polish(ctx, self.dark_mode);
        self.cfg.theme = if self.dark_mode { "dark".into() } else { "light".into() };
        let _ = save_config(&self.cfg);
    }

    fn apply_accent(&mut self, ctx: &egui::Context) {
        set_accent(if self.accent_red { ACCENT_RED } else { ACCENT });
        apply_polish(ctx, self.dark_mode);
        self.cfg.accent = if self.accent_red { "red".into() } else { "teal".into() };
        let _ = save_config(&self.cfg);
    }
}

impl eframe::App for VrDoctorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("🩺 VRDoctor").size(23.0).strong().color(accent()));
                ui.label("PCVR on Meta Quest 3 — Arch & Fedora Linux");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.checkbox(&mut self.dark_mode, "🌙 Dark mode").changed() {
                        self.apply_theme(ctx);
                    }
                    ui.label("Accent:");
                    if ui.selectable_value(&mut self.accent_red, true, "Red").changed() { self.apply_accent(ctx); }
                    if ui.selectable_value(&mut self.accent_red, false, "Teal").changed() { self.apply_accent(ctx); }
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
                ui.selectable_value(&mut self.tab, Tab::Settings, "🎛 Settings");
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
                Tab::Settings => self.settings_tab(ui),
            }
        });
    }
}

impl VrDoctorApp {
    fn games_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("🎮 Library");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.selectable_value(&mut self.grid_view, false, "☰ List");
                ui.selectable_value(&mut self.grid_view, true, "▦ Grid");
                if ui.button("🎨 Fetch cover art").clicked() {
                    spawn_art_download(ui.ctx(), self.steam_games.iter().map(|g| g.appid).collect());
                    self.status = "Fetching cover art from Steam in the background…".into();
                }
                if ui.button("🔍 Check Steam online (VR or 2D?)").clicked() {
                    let mut known = 0;
                    for g in &self.steam_games { let r = classify_game(g, true); if r.0 != VrSupport::Unknown { known += 1; } self.vr_info.insert(g.appid, r); }
                    self.status = format!("Checked Steam: {known}/{} games classified. Badges: VR = VR-only · VR+2D = plays both ways · 2D = flat game · ? = not checked.", self.steam_games.len());
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut self.search);
        });
        let needle = self.search.to_lowercase();
        self.detail_bar(ui);
        ui.separator();
        if self.grid_view { self.library_grid(ui, &needle); } else { self.library_list(ui, &needle); }
        if let Some(p) = self.pending.take() {
            match p {
                Pending::Steam(appid) => { self.status = match launch_steam_appid(appid, &self.backend.clone()) { Ok(m) => m, Err(e) => e }; }
                Pending::Steam2d(appid) => { self.status = match launch_steam_2d(appid, &self.backend.clone()) { Ok(m) => m, Err(e) => e }; }
                Pending::Custom(mut g) => {
                    if g.backend.is_empty() { g.backend = self.backend.clone(); }
                    self.status = match launch_custom_game(&g, &self.cfg) { Ok(m) => m, Err(e) => e };
                }
            }
        }
    }

    /// Steam-style detail strip for the currently selected game.
    fn detail_bar(&mut self, ui: &mut egui::Ui) {
        if let Some(appid) = self.selected_steam {
            if let Some(g) = self.steam_games.iter().find(|g| g.appid == appid).cloned() {
                let (sup, src) = self.class_of(&g);
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(&g.name).size(19.0).strong());
                            ui.horizontal(|ui| {
                                badge(ui, sup);
                                ui.small(format!("AppID {} · detected via {} · backend: {}", g.appid, src, self.backend));
                            });
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✕").clicked() { self.selected_steam = None; }
                            if ui.button("▶ 2D").clicked() { self.pending = Some(Pending::Steam2d(g.appid)); }
                            let launch = egui::Button::new(egui::RichText::new("▶ Launch").strong().color(egui::Color32::from_rgb(6, 24, 22))).fill(accent());
                            if ui.add(launch).clicked() { self.pending = Some(Pending::Steam(g.appid)); }
                        });
                    });
                });
            }
        } else if let Some(name) = self.selected_custom.clone() {
            if let Some(g) = self.custom_games.iter().find(|g| g.name == name).cloned() {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(&g.name).size(19.0).strong());
                            ui.small(format!("Custom game · {}{}", g.path.display(), if g.mode == "2d" { " · plays in 2D mode" } else { "" }));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✕").clicked() { self.selected_custom = None; }
                            let launch = egui::Button::new(egui::RichText::new("▶ Launch").strong().color(egui::Color32::from_rgb(6, 24, 22))).fill(accent());
                            if ui.add(launch).clicked() { self.pending = Some(Pending::Custom(g.clone())); }
                        });
                    });
                });
            }
        }
    }

    /// The Steam-style cover-art grid.
    fn library_grid(&mut self, ui: &mut egui::Ui, needle: &str) {
        let custom: Vec<CustomGame> = self.custom_games.iter().filter(|g| needle.is_empty() || g.name.to_lowercase().contains(needle)).cloned().collect();
        let steam: Vec<Game> = self.steam_games.iter().filter(|g| needle.is_empty() || g.name.to_lowercase().contains(needle)).cloned().collect();
        egui::ScrollArea::vertical().show(ui, |ui| {
            if !custom.is_empty() {
                ui.label(egui::RichText::new("Custom games").size(17.0).strong().color(accent()));
                ui.horizontal(|ui| { for g in &custom { self.custom_tile(ui, g); } });
                ui.add_space(8.0);
            }
            ui.label(egui::RichText::new("Steam games").size(17.0).strong().color(accent()));
            ui.small("Cover art comes from Steam. Click a game for details; ▶ launches, 2D plays it on the big screen.");
            if steam.is_empty() { ui.label("No Steam games found. Check Doctor — Steam may not be installed yet."); }
            let cols = (((ui.available_width() + 14.0) / 176.0) as usize).max(1);
            for chunk in steam.chunks(cols) {
                ui.horizontal(|ui| { for g in chunk { self.steam_tile(ui, g); } });
                ui.add_space(8.0);
            }
        });
    }

    fn steam_tile(&mut self, ui: &mut egui::Ui, g: &Game) {
        ui.vertical(|ui| {
            ui.set_width(162.0);
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(150.0, 225.0), egui::Sense::click());
            match self.texture_for(ui.ctx(), g.appid) {
                Some(tex) => { ui.put(rect, egui::Image::new(&tex).fit_to_exact_size(rect.size())); }
                None => {
                    let p = ui.painter_at(rect);
                    p.rect_filled(rect, egui::Rounding::same(8.0), tile_color(g.appid));
                    let letter = g.name.chars().find(|c| c.is_alphanumeric()).unwrap_or('?').to_uppercase().to_string();
                    p.text(rect.center() - egui::vec2(0.0, 16.0), egui::Align2::CENTER_CENTER, letter, egui::FontId::proportional(64.0), egui::Color32::from_white_alpha(220));
                    p.text(rect.center() + egui::vec2(0.0, 54.0), egui::Align2::CENTER_CENTER, "VRDoctor", egui::FontId::proportional(13.0), egui::Color32::from_white_alpha(140));
                }
            }
            if self.selected_steam == Some(g.appid) {
                ui.painter().rect_stroke(rect, egui::Rounding::same(8.0), egui::Stroke::new(2.5, accent()));
            }
            if resp.clicked() { self.selected_steam = Some(g.appid); self.selected_custom = None; }
            ui.label(egui::RichText::new(truncate_name(&g.name, 24)).strong());
            ui.horizontal(|ui| {
                let (sup, _) = self.class_of(g);
                badge(ui, sup);
                if ui.small_button("▶ Launch").clicked() { self.pending = Some(Pending::Steam(g.appid)); }
                if ui.small_button("2D").clicked() { self.pending = Some(Pending::Steam2d(g.appid)); }
            });
        });
    }

    fn custom_tile(&mut self, ui: &mut egui::Ui, g: &CustomGame) {
        ui.vertical(|ui| {
            ui.set_width(162.0);
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(150.0, 225.0), egui::Sense::click());
            let seed = g.name.bytes().fold(0u64, |a, b| a.wrapping_add(b as u64).wrapping_mul(31));
            let p = ui.painter_at(rect);
            p.rect_filled(rect, egui::Rounding::same(8.0), tile_color(seed));
            let letter = g.name.chars().find(|c| c.is_alphanumeric()).unwrap_or('?').to_uppercase().to_string();
            p.text(rect.center() - egui::vec2(0.0, 16.0), egui::Align2::CENTER_CENTER, letter, egui::FontId::proportional(64.0), egui::Color32::from_white_alpha(220));
            p.text(rect.center() + egui::vec2(0.0, 54.0), egui::Align2::CENTER_CENTER, if g.mode == "2d" { "2D · custom" } else { "custom game" }, egui::FontId::proportional(13.0), egui::Color32::from_white_alpha(160));
            if self.selected_custom.as_deref() == Some(g.name.as_str()) {
                ui.painter().rect_stroke(rect, egui::Rounding::same(8.0), egui::Stroke::new(2.5, accent()));
            }
            if resp.clicked() { self.selected_custom = Some(g.name.clone()); self.selected_steam = None; }
            ui.label(egui::RichText::new(truncate_name(&g.name, 24)).strong());
            ui.horizontal(|ui| {
                if ui.small_button("▶ Launch").clicked() { self.pending = Some(Pending::Custom(g.clone())); }
            });
        });
    }

    /// The classic card list (kept as the List view).
    fn library_list(&mut self, ui: &mut egui::Ui, needle: &str) {
        ui.heading("Custom games (not from a store)");
        if self.custom_games.is_empty() {
            ui.label("None yet — use the “Add custom game” tab to add any local game, store or not.");
        }
        let mut to_launch: Option<CustomGame> = None;
        let mut to_remove: Option<String> = None;
        for g in self.custom_games.iter().filter(|g| needle.is_empty() || g.name.to_lowercase().contains(needle)) {
            card(ui, |ui| { ui.horizontal(|ui| {
                if ui.button("▶ Launch").clicked() { to_launch = Some(g.clone()); }
                ui.vertical(|ui| {
                    ui.strong(format!("{}{}", g.name, if g.mode == "2d" { "  [2D]" } else { "" }));
                    ui.small(format!("{}{}", g.path.display(), if g.args.is_empty() { String::new() } else { format!("  {}", g.args) }));
                });
                if ui.button("🗑").clicked() { to_remove = Some(g.name.clone()); }
            }); });
        }
        if let Some(g) = to_launch { self.pending = Some(Pending::Custom(g)); }
        if let Some(name) = to_remove {
            self.status = match remove_custom_game(&name) {
                Ok(true) => format!("Removed custom game '{name}'."),
                Ok(false) => format!("No custom game named '{name}'."),
                Err(e) => format!("Failed to remove: {e}"),
            };
            self.refresh_games();
        }

        ui.separator();
        ui.heading("Steam games");
        ui.small("VRDoctor auto-detects whether each Steam game is VR or 2D. Launching a 2D game sends it to the big screen automatically.");
        if self.steam_games.is_empty() {
            ui.label("No Steam games found. Check Doctor — Steam may not be installed yet.");
        }
        let mut steam_launch: Option<Game> = None;
        let mut steam_launch_2d: Option<Game> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for g in self.steam_games.iter().filter(|g| needle.is_empty() || g.name.to_lowercase().contains(needle)) {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("▶ Launch").clicked() { steam_launch = Some(g.clone()); }
                        if ui.button("▶ 2D").clicked() { steam_launch_2d = Some(g.clone()); }
                        let (sup, _src) = self.class_of(g);
                        badge(ui, sup);
                        ui.strong(&g.name);
                        ui.small(format!("AppID {}", g.appid));
                    });
                });
            }
        });
        if let Some(g) = steam_launch { self.pending = Some(Pending::Steam(g.appid)); }
        if let Some(g) = steam_launch_2d { self.pending = Some(Pending::Steam2d(g.appid)); }
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


    fn save_cfg(&mut self) { let _ = save_config(&self.cfg); }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.small("Changes save automatically and are shared with VRDoctor VR in the headset.");
        ui.separator();

        ui.label(egui::RichText::new("Appearance").size(17.0).strong().color(accent()));
        ui.horizontal(|ui| {
            if ui.checkbox(&mut self.dark_mode, "🌙 Dark mode").changed() { self.apply_theme(ui.ctx()); }
            ui.label("Accent:");
            if ui.selectable_value(&mut self.accent_red, true, "Red").changed() { self.apply_accent(ui.ctx()); }
            if ui.selectable_value(&mut self.accent_red, false, "Teal").changed() { self.apply_accent(ui.ctx()); }
        });
        ui.separator();

        ui.label(egui::RichText::new("Streaming & VR").size(17.0).strong().color(accent()));
        ui.horizontal(|ui| {
            ui.label("Backend:");
            let mut backend = self.cfg.backend.clone();
            for (b, label) in [("wivrn", "WiVRn"), ("alvr", "ALVR"), ("steamlink", "Steam Link")] {
                if ui.selectable_value(&mut backend, b.to_string(), label).changed() {
                    self.cfg.backend = backend.clone();
                    self.backend = backend.clone();
                    self.save_cfg();
                }
            }
        });
        let mut hz = self.cfg.default_refresh as i32;
        if ui.add(egui::Slider::new(&mut hz, 72..=120).text("Refresh rate (Hz)")).changed() {
            self.cfg.default_refresh = hz as u32;
            self.save_cfg();
        }
        ui.small("VRDoctor VR asks the runtime for this rate. For games, the rate is picked in the WiVRn app on the Quest.");
        let mut pct = (self.cfg.render_scale * 100.0).round() as i32;
        if ui.add(egui::Slider::new(&mut pct, 50..=200).text("VR resolution (%)")).changed() {
            self.cfg.render_scale = pct as f32 / 100.0;
            self.save_cfg();
        }
        ui.small("How sharp VRDoctor VR itself renders. Higher looks crisper but costs GPU; lower is smoother. (Game streaming resolution lives in the WiVRn app on the Quest.)");
        ui.separator();

        ui.label(egui::RichText::new("Environment (VR Home)").size(17.0).strong().color(accent()));
        ui.small("The world you stand in inside VRDoctor VR.");
        ui.horizontal_wrapped(|ui| {
            for t in HOME_THEMES {
                let label = t[..1].to_uppercase() + &t[1..];
                if ui.selectable_label(self.set_theme == *t, label).clicked() { self.set_theme = t.to_string(); }
            }
        });
        let mut bright = (self.set_brightness * 100.0).round() as i32;
        if ui.add(egui::Slider::new(&mut bright, 50..=160).text("Brightness (%)")).changed() {
            self.set_brightness = bright as f32 / 100.0;
        }
        if ui.button("🌅 Apply environment").clicked() {
            match home_install(&self.set_theme, self.set_brightness) {
                Ok(m) => {
                    self.cfg.home_theme = self.set_theme.clone();
                    self.cfg.home_brightness = self.set_brightness;
                    self.save_cfg();
                    self.status = m;
                }
                Err(e) => self.status = e,
            }
        }
        ui.separator();

        ui.label(egui::RichText::new("VR display fixes").size(17.0).strong().color(accent()));
        if ui.checkbox(&mut self.cfg.flip_panel, "Panel appears upside down — flip it").changed() { self.save_cfg(); }
        if ui.checkbox(&mut self.cfg.flip_env, "Environment appears upside down — flip it").changed() { self.save_cfg(); }
        ui.small("You can change these inside the headset too: open VRDoctor VR and tap ⚙ Settings.");
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
        ui.heading("Uninstall");
        ui.label("Remove VRDoctor's own files: settings, custom games, VR Home, cover-art cache and menu entries. The VR packages it installed (WiVRn and friends) are shared system packages and are left in place — remove them with your package manager if you want them gone too (Arch: wivrn-dashboard · Fedora: wivrn). Afterwards, delete the VRDoctor AppImage file itself and the app is fully gone.");
        if !self.confirm_uninstall {
            if ui.button("🗑 Uninstall VRDoctor…").clicked() { self.confirm_uninstall = true; }
        } else {
            ui.colored_label(egui::Color32::from_rgb(248, 113, 113), "This deletes your VRDoctor settings and custom game list. There is no undo.");
            ui.horizontal(|ui| {
                if ui.button("Yes, remove VRDoctor's files").clicked() {
                    let report = uninstall_user_files();
                    self.status = format!("Uninstall: {}", report.join(" · "));
                    self.confirm_uninstall = false;
                }
                if ui.button("Cancel").clicked() { self.confirm_uninstall = false; }
            });
        }
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
        set_accent_from_config(&cfg);
        apply_polish(&cc.egui_ctx, dark_mode);
        let distro = current_distro();
        let family = match distro.family { Family::Arch | Family::Fedora => distro.family.clone(), _ => Family::Arch };
        Self { family, detected: distro.pretty, status: "Pick your distro, review the plan, then install.".into() }
    }
}

impl eframe::App for InstallerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(egui::RichText::new("🩺 VRDoctor installer").size(23.0).strong().color(accent()));
            ui.label(format!("Detected: {}. If that's wrong, just pick the right one:", self.detected));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.family, Family::Arch, "🟦 Arch Linux");
                ui.selectable_value(&mut self.family, Family::Fedora, "🟥 Fedora");
            });
            ui.add_space(8.0);
            ui.label(format!("Install plan for {:?}:", self.family));
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for line in install_plan(&self.family) { ui.monospace(line); }
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
        set_accent_from_config(&load_config());
        apply_polish(&cc.egui_ctx, true); // the bar is always dark, like Quest's
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
                .fill(egui::Color32::from_rgb(17, 24, 39))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 212, 191)))
                .rounding(egui::Rounding::same(28.0))
                .inner_margin(egui::Margin::symmetric(18.0, 8.0))
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.label(egui::RichText::new("🩺 VRDoctor").strong().color(accent()).size(16.0));
                        ui.separator();
                        ui.label(egui::RichText::new(format!("🕐 {clock}")).strong().size(19.0));
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
