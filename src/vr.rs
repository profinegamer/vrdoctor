//! vrdoctor-vr — the VRDoctor library, inside the headset.
//!
//! A real OpenXR application (launched by WiVRn, whose launcher lists any
//! .desktop file with `X-WiVRn-VR` in Categories — `vrdoctor vr` writes
//! that entry). It shows your games as a Frame-style floating panel with
//! Steam cover art; point a controller at a game and pull the trigger to
//! launch it through the normal `vrdoctor launch` path, then this app
//! politely exits so the game can take over the runtime.
//!
//! Rendering: OpenGL via an EGL context (surfaceless Mesa where possible)
//! bound with XR_MNDX_egl_enable — the combination Monado/WiVRn support
//! on Linux desktops regardless of X11 vs Wayland. The panel itself is
//! rasterised on the CPU (fontdue text, `image` for cover art) and
//! submitted as a crisp composition quad layer over a panorama scene
//! rendered per-eye (your VR Home panorama when one is installed).
//!
//! Hidden test hooks: `--probe` (print runtime/system info, exit 0),
//! `--frames N` (auto-exit after N rendered frames), `--dump-panel PATH`
//! (write the panel raster as a PNG without touching OpenXR).

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

type VResult<T> = Result<T, String>;

// ---------------------------------------------------------------- data

#[derive(Debug, Clone)]
struct VrGame {
    kind: String, // "steam" | "custom"
    appid: u64,
    name: String,
    vr: String, // "vr" | "vr2d" | "2d" | "unknown"
}

/// A binary sitting next to this executable (falling back to PATH lookup).
fn sibling(name: &str) -> PathBuf {
    if let Ok(exe) = env::current_exe() {
        let p = exe.with_file_name(name);
        if p.is_file() { return p; }
    }
    PathBuf::from(name)
}

/// Extract a string field from one flat JSON object (our own emitter's
/// shape — see games_json in main.rs). Handles the standard escapes.
fn json_str_field(obj: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let at = obj.find(&pat)?;
    let rest = obj[at + pat.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    let mut cp = u32::from_str_radix(&hex, 16).ok()?;
                    // Combine a surrogate pair if one follows.
                    if (0xD800..0xDC00).contains(&cp) {
                        let save = chars.clone();
                        if chars.next() == Some('\\') && chars.next() == Some('u') {
                            let hex2: String = chars.by_ref().take(4).collect();
                            if let Ok(lo) = u32::from_str_radix(&hex2, 16) {
                                if (0xDC00..0xE000).contains(&lo) {
                                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                } else { chars = save; }
                            } else { chars = save; }
                        } else { chars = save; }
                    }
                    out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                }
                _ => return None,
            },
            c => out.push(c),
        }
    }
}

fn json_num_field(obj: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\"");
    let at = obj.find(&pat)?;
    let rest = obj[at + pat.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Split a JSON array of flat objects into per-object substrings.
fn json_objects(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut depth = 0i32;
    let mut start = None;
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in s.char_indices() {
        if in_str {
            if esc { esc = false; } else if c == '\\' { esc = true; } else if c == '"' { in_str = false; }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => { if depth == 0 { start = Some(i); } depth += 1; }
            '}' => { depth -= 1; if depth == 0 { if let Some(st) = start.take() { out.push(s[st..=i].to_string()); } } }
            _ => {}
        }
    }
    out
}

fn parse_games_json(s: &str) -> Vec<VrGame> {
    json_objects(s).iter().filter_map(|obj| {
        Some(VrGame {
            kind: json_str_field(obj, "kind")?,
            appid: json_num_field(obj, "appid").unwrap_or(0),
            name: json_str_field(obj, "name")?,
            vr: json_str_field(obj, "vr").unwrap_or_else(|| "unknown".into()),
        })
    }).collect()
}

/// The game list, from the sibling vrdoctor binary. Sorted the way the
/// panel wants them: VR first, then hybrid, unknown, then flat 2D.
fn load_games() -> Vec<VrGame> {
    let mut games = Command::new(sibling("vrdoctor"))
        .args(["list-games", "--json"])
        .output()
        .map(|o| parse_games_json(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    let rank = |g: &VrGame| match g.vr.as_str() { "vr" => 0, "vr2d" => 1, "unknown" => 2, _ => 3 };
    games.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    games
}

// ---------------------------------------------------------------- paths

fn cache_base() -> PathBuf {
    env::var("XDG_CACHE_HOME").map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".cache"))
}
fn art_path(appid: u64) -> PathBuf { cache_base().join("vrdoctor/art").join(format!("{appid}.jpg")) }

fn config_base() -> PathBuf {
    env::var("XDG_CONFIG_HOME").map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".config"))
}

/// The installed VR Home panorama (dawn preferred), if any.
fn find_panorama() -> Option<PathBuf> {
    let dir = config_base().join("vrdoctor/home");
    let dawn = dir.join("vrdoctor-home-dawn.png");
    if dawn.is_file() { return Some(dawn); }
    let mut found: Vec<PathBuf> = fs::read_dir(&dir).ok()?
        .flatten().map(|e| e.path())
        .filter(|p| p.file_name().map(|n| {
            let n = n.to_string_lossy();
            n.starts_with("vrdoctor-home-") && n.ends_with(".png")
        }).unwrap_or(false))
        .collect();
    found.sort();
    found.into_iter().next()
}

// ---------------------------------------------------------------- raster

type Rgba = [u8; 4];
const TEAL: Rgba = [45, 212, 191, 255];
const RED: Rgba = [239, 68, 68, 255];

/// The user's accent colour (config `accent=teal|red`), read once.
fn accent() -> Rgba {
    use std::sync::OnceLock;
    static A: OnceLock<Rgba> = OnceLock::new();
    *A.get_or_init(|| {
        let text = std::fs::read_to_string(config_base().join("vrdoctor/config.conf")).unwrap_or_default();
        if text.lines().any(|l| l.trim() == "accent=red") { RED } else { TEAL }
    })
}
const SLATE: Rgba = [13, 18, 32, 255];
const WHITE: Rgba = [255, 255, 255, 255];

struct Raster {
    w: usize,
    h: usize,
    px: Vec<u8>, // RGBA, row-major, top row first
}

impl Raster {
    fn new(w: usize, h: usize) -> Self { Raster { w, h, px: vec![0; w * h * 4] } }

    fn blend(&mut self, x: i32, y: i32, c: Rgba, coverage: f32) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 { return; }
        let a = (c[3] as f32 / 255.0) * coverage.clamp(0.0, 1.0);
        if a <= 0.0 { return; }
        let i = (y as usize * self.w + x as usize) * 4;
        if a >= 1.0 { self.px[i..i + 4].copy_from_slice(&c); return; }
        let ia = 1.0 - a;
        for k in 0..3 { self.px[i + k] = (c[k] as f32 * a + self.px[i + k] as f32 * ia) as u8; }
        self.px[i + 3] = (255.0 * a + self.px[i + 3] as f32 * ia) as u8;
    }

    fn set(&mut self, x: i32, y: i32, c: Rgba) { self.blend(x, y, c, 1.0); }

    /// Signed distance to a rounded rect centred in the given box.
    fn rounded_sdf(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
        let cx = x + w / 2.0;
        let cy = y + h / 2.0;
        let qx = (px - cx).abs() - (w / 2.0 - r);
        let qy = (py - cy).abs() - (h / 2.0 - r);
        let ax = qx.max(0.0);
        let ay = qy.max(0.0);
        (ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - r
    }

    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgba) {
        let x0 = x.round() as i32;
        let y0 = y.round() as i32;
        let x1 = (x + w).round() as i32;
        let y1 = (y + h).round() as i32;
        for py in y0..y1 {
            for px in x0..x1 {
                self.blend(px, py, c, 1.0);
            }
        }
    }

    fn fill_rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgba) {
        if r <= 0.5 { self.fill_rect(x, y, w, h, c); return; }
        // Fast path: the three interior rectangles (no SDF needed), then
        // SDF only in the four corner squares.
        self.fill_rect(x + r, y, w - 2.0 * r, h, c);
        self.fill_rect(x, y + r, r, h - 2.0 * r, c);
        self.fill_rect(x + w - r, y + r, r, h - 2.0 * r, c);
        let corners = [
            (x, y), (x + w - r - 1.0, y), (x, y + h - r - 1.0), (x + w - r - 1.0, y + h - r - 1.0),
        ];
        for (cx, cy) in corners {
            let x0 = cx.floor() as i32;
            let y0 = cy.floor() as i32;
            let x1 = (cx + r + 2.0).ceil() as i32;
            let y1 = (cy + r + 2.0).ceil() as i32;
            for py in y0..y1 {
                for px in x0..x1 {
                    let d = Self::rounded_sdf(px as f32 + 0.5, py as f32 + 0.5, x, y, w, h, r);
                    let cov = (0.5 - d).clamp(0.0, 1.0);
                    if cov > 0.0 { self.blend(px, py, c, cov); }
                }
            }
        }
    }

    fn stroke_rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, t: f32, c: Rgba) {
        // Only the border band can be covered — iterate four strips.
        let pad = t + 1.0;
        let bands = [
            (x - pad, y - pad, w + 2.0 * pad, 2.0 * pad + t),          // top
            (x - pad, y + h - t - pad, w + 2.0 * pad, 2.0 * pad + t),  // bottom
            (x - pad, y, 2.0 * pad + t, h),                            // left
            (x + w - t - pad, y, 2.0 * pad + t, h),                    // right
        ];
        for (bx, by, bw, bh) in bands {
            let x0 = bx.floor() as i32;
            let y0 = by.floor() as i32;
            let x1 = (bx + bw).ceil() as i32;
            let y1 = (by + bh).ceil() as i32;
            for py in y0..y1 {
                for px in x0..x1 {
                    let d = Self::rounded_sdf(px as f32 + 0.5, py as f32 + 0.5, x, y, w, h, r);
                    let cov = (t / 2.0 - (d + t / 2.0).abs() + 0.5).clamp(0.0, 1.0);
                    if cov > 0.0 { self.blend(px, py, c, cov); }
                }
            }
        }
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, c: Rgba) {
        let x0 = (cx - r - 1.0).floor() as i32;
        let y0 = (cy - r - 1.0).floor() as i32;
        let x1 = (cx + r + 1.0).ceil() as i32;
        let y1 = (cy + r + 1.0).ceil() as i32;
        for py in y0..y1 {
            for px in x0..x1 {
                let dx = px as f32 + 0.5 - cx;
                let dy = py as f32 + 0.5 - cy;
                let cov = (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
                if cov > 0.0 { self.blend(px, py, c, cov); }
            }
        }
    }

    /// Blit an image (already sized to w×h) with rounded-corner masking.
    /// Interior pixels skip the SDF entirely.
    fn blit_rounded(&mut self, img: &image::RgbaImage, x: i32, y: i32, r: f32) {
        let (iw, ih) = img.dimensions();
        let ri = r as i32 + 1;
        for dy in 0..ih as i32 {
            for dx in 0..iw as i32 {
                let p = img.get_pixel(dx as u32, dy as u32).0;
                let interior = dx >= ri && dx < iw as i32 - ri && dy >= ri && dy < ih as i32 - ri;
                if interior {
                    self.set(x + dx, y + dy, p);
                } else {
                    let d = Self::rounded_sdf(x as f32 + dx as f32 + 0.5, y as f32 + dy as f32 + 0.5,
                                              x as f32, y as f32, iw as f32, ih as f32, r);
                    let cov = (0.5 - d).clamp(0.0, 1.0);
                    if cov > 0.0 { self.blend(x + dx, y + dy, p, cov); }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- text

struct Fonts {
    regular: fontdue::Font,
    bold: fontdue::Font,
}

impl Fonts {
    fn load() -> VResult<Fonts> {
        let regular = fontdue::Font::from_bytes(
            include_bytes!("../assets/DejaVuSans.ttf") as &[u8], fontdue::FontSettings::default())
            .map_err(|e| format!("regular font: {e}"))?;
        let bold = fontdue::Font::from_bytes(
            include_bytes!("../assets/DejaVuSans-Bold.ttf") as &[u8], fontdue::FontSettings::default())
            .map_err(|e| format!("bold font: {e}"))?;
        Ok(Fonts { regular, bold })
    }

    fn font(&self, bold: bool) -> &fontdue::Font { if bold { &self.bold } else { &self.regular } }

    fn measure(&self, text: &str, size: f32, bold: bool) -> f32 {
        let f = self.font(bold);
        text.chars().map(|c| f.rasterize(c, size).0.advance_width).sum()
    }

    fn ascent(&self, size: f32, bold: bool) -> f32 {
        self.font(bold).horizontal_line_metrics(size).map(|m| m.ascent).unwrap_or(size * 0.8)
    }

    /// Truncate with an ellipsis so the text fits max_w at this size.
    fn truncate(&self, text: &str, size: f32, bold: bool, max_w: f32) -> String {
        if self.measure(text, size, bold) <= max_w { return text.to_string(); }
        let mut out = String::new();
        for c in text.chars() {
            if self.measure(&format!("{out}{c}…"), size, bold) > max_w { break; }
            out.push(c);
        }
        out.push('…');
        out
    }

    /// Draw text with its baseline at (x, baseline). Returns the pen advance.
    fn draw(&self, r: &mut Raster, text: &str, x: f32, baseline: f32, size: f32, bold: bool, color: Rgba) -> f32 {
        let f = self.font(bold);
        let mut pen = x;
        for c in text.chars() {
            let (m, cov) = f.rasterize(c, size);
            let gx = pen + m.xmin as f32;
            let gy = baseline - (m.ymin as f32 + m.height as f32);
            for row in 0..m.height {
                for col in 0..m.width {
                    let a = cov[row * m.width + col] as f32 / 255.0;
                    if a > 0.0 {
                        r.blend((gx + col as f32).round() as i32, (gy + row as f32).round() as i32, color, a);
                    }
                }
            }
            pen += m.advance_width;
        }
        pen - x
    }

    fn draw_centered(&self, r: &mut Raster, text: &str, cx: f32, baseline: f32, size: f32, bold: bool, color: Rgba) {
        let w = self.measure(text, size, bold);
        self.draw(r, text, cx - w / 2.0, baseline, size, bold, color);
    }
}

// ---------------------------------------------------------------- panel UI

pub const PANEL_W: usize = 2048;
pub const PANEL_H: usize = 1280;
const PAGE_SIZE: usize = 8;
const ART_W: u32 = 280;
const ART_H: u32 = 420;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect { x: f32, y: f32, w: f32, h: f32 }
impl Rect {
    fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Region {
    Launch(usize), // index into the full game list
    PagePrev,
    PageNext,
    Quit,
}

/// Tile cell rect for slot i (0..PAGE_SIZE) — shared by render + hit test.
fn cell_rect(slot: usize) -> Rect {
    let col = (slot % 4) as f32;
    let row = (slot / 4) as f32;
    Rect { x: 56.0 + col * (466.0 + 24.0), y: 176.0 + row * (452.0 + 20.0), w: 466.0, h: 452.0 }
}
fn art_rect(slot: usize) -> Rect {
    let c = cell_rect(slot);
    Rect { x: c.x + (c.w - ART_W as f32) / 2.0, y: c.y + 4.0, w: ART_W as f32, h: ART_H as f32 }
}
fn quit_rect() -> Rect { Rect { x: PANEL_W as f32 - 56.0 - 190.0, y: 44.0, w: 190.0, h: 64.0 } }
fn prev_rect() -> Rect { Rect { x: 56.0, y: 1128.0, w: 120.0, h: 56.0 } }
fn next_rect() -> Rect { Rect { x: PANEL_W as f32 - 56.0 - 120.0, y: 1128.0, w: 120.0, h: 56.0 } }

fn badge_of(vr: &str) -> (&'static str, Rgba) {
    match vr {
        "vr" => ("VR", [21, 101, 52, 255]),
        "vr2d" => ("VR+2D", [88, 28, 175, 255]),
        "2d" => ("2D", [29, 64, 175, 255]),
        _ => ("?", [133, 77, 14, 255]),
    }
}

fn tile_color(seed: u64) -> Rgba {
    const PALETTE: [(u8, u8, u8); 8] = [
        (30, 64, 175), (21, 101, 52), (88, 28, 175), (154, 52, 18),
        (14, 116, 144), (134, 25, 143), (133, 77, 14), (51, 65, 85),
    ];
    let (r, g, b) = PALETTE[(seed % 8) as usize];
    [r, g, b, 255]
}

struct Panel {
    fonts: Fonts,
    games: Vec<VrGame>,
    page: usize,
    status: String,
    status_is_error: bool,
    art_cache: HashMap<usize, Option<image::RgbaImage>>, // by game index, pre-sized to ART_W×ART_H
    regions: Vec<(Rect, Region)>,
}

impl Panel {
    fn new(games: Vec<VrGame>) -> VResult<Panel> {
        Ok(Panel {
            fonts: Fonts::load()?,
            games,
            page: 0,
            status: String::new(),
            status_is_error: false,
            art_cache: HashMap::new(),
            regions: vec![],
        })
    }

    fn pages(&self) -> usize { ((self.games.len() + PAGE_SIZE - 1) / PAGE_SIZE).max(1) }

    fn tile_art(&mut self, idx: usize) -> Option<image::RgbaImage> {
        if let Some(a) = self.art_cache.get(&idx) { return a.clone(); }
        let g = &self.games[idx];
        let art = if g.kind == "steam" && g.appid > 0 {
            let path = art_path(g.appid);
            if path.is_file() {
                image::open(&path).ok().map(|img| {
                    // Cover-fit into ART_W×ART_H (crop to 2:3, then resize).
                    let rgba = img.to_rgba8();
                    let (sw, sh) = rgba.dimensions();
                    let target = ART_W as f32 / ART_H as f32;
                    let src = sw as f32 / sh as f32;
                    let (cw, ch) = if src > target {
                        ((sh as f32 * target) as u32, sh)
                    } else {
                        (sw, (sw as f32 / target) as u32)
                    };
                    let cropped = image::imageops::crop_imm(&rgba, (sw - cw) / 2, (sh - ch) / 2, cw, ch).to_image();
                    image::imageops::resize(&cropped, ART_W, ART_H, image::imageops::FilterType::Triangle)
                })
            } else { None }
        } else { None };
        self.art_cache.insert(idx, art.clone());
        art
    }

    /// Render the whole panel. `hover` holds the region under each hand's
    /// laser (if any), `dots` the laser hit points in raster coordinates.
    fn render(&mut self, hover: [Option<Region>; 2], dots: [Option<(f32, f32)>; 2]) -> Raster {
        let mut r = Raster::new(PANEL_W, PANEL_H);
        self.regions.clear();
        // Panel body + hairline border.
        r.fill_rounded_rect(2.0, 2.0, PANEL_W as f32 - 4.0, PANEL_H as f32 - 4.0, 36.0, [SLATE[0], SLATE[1], SLATE[2], 243]);
        r.stroke_rounded_rect(2.0, 2.0, PANEL_W as f32 - 4.0, PANEL_H as f32 - 4.0, 36.0, 2.0, [255, 255, 255, 26]);

        // Header: teal logo tile with a white medical cross, then titles.
        r.fill_rounded_rect(56.0, 44.0, 72.0, 72.0, 16.0, accent());
        r.fill_rounded_rect(56.0 + 31.0, 44.0 + 16.0, 10.0, 40.0, 3.0, WHITE);
        r.fill_rounded_rect(56.0 + 16.0, 44.0 + 31.0, 40.0, 10.0, 3.0, WHITE);
        let title_base = 48.0 + self.fonts.ascent(56.0, true);
        self.fonts.draw(&mut r, "VRDoctor", 148.0, title_base, 56.0, true, WHITE);
        let sub_base = 112.0 + self.fonts.ascent(27.0, false);
        self.fonts.draw(&mut r, "Your library in VR — point at a game and pull the trigger",
                        148.0, sub_base, 27.0, false, [148, 163, 184, 255]);

        // Quit button.
        let q = quit_rect();
        let q_hover = hover.contains(&Some(Region::Quit));
        r.fill_rounded_rect(q.x, q.y, q.w, q.h, 14.0, if q_hover { [51, 65, 85, 255] } else { [30, 41, 59, 255] });
        let qb = q.y + 20.0 + self.fonts.ascent(30.0, true);
        self.fonts.draw_centered(&mut r, "× Quit", q.x + q.w / 2.0, qb, 30.0, true, WHITE);
        self.regions.push((q, Region::Quit));

        // Games grid (or the empty message).
        if self.games.is_empty() {
            let base = 640.0 + self.fonts.ascent(34.0, false);
            self.fonts.draw_centered(&mut r, "No games found.", PANEL_W as f32 / 2.0, base - 30.0, 34.0, true, WHITE);
            self.fonts.draw_centered(&mut r, "Add some in VRDoctor on your PC, then come back.",
                                     PANEL_W as f32 / 2.0, base + 30.0, 28.0, false, [148, 163, 184, 255]);
        }
        let first = self.page * PAGE_SIZE;
        for slot in 0..PAGE_SIZE {
            let idx = first + slot;
            if idx >= self.games.len() { break; }
            let g = self.games[idx].clone();
            let cell = cell_rect(slot);
            let art = art_rect(slot);
            let region = Region::Launch(idx);
            let hovered = hover.contains(&Some(region));
            match self.tile_art(idx) {
                Some(img) => r.blit_rounded(&img, art.x as i32, art.y as i32, 14.0),
                None => {
                    let seed = if g.appid > 0 { g.appid } else {
                        g.name.bytes().fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64))
                    };
                    r.fill_rounded_rect(art.x, art.y, art.w, art.h, 14.0, tile_color(seed));
                    let letter = g.name.chars().find(|c| c.is_alphanumeric()).unwrap_or('?')
                        .to_uppercase().to_string();
                    let lb = art.y + art.h / 2.0 - 20.0 + self.fonts.ascent(150.0, true) / 2.0;
                    self.fonts.draw_centered(&mut r, &letter, art.x + art.w / 2.0, lb, 150.0, true, [255, 255, 255, 220]);
                    let wb = art.y + art.h / 2.0 + 78.0 + self.fonts.ascent(30.0, false);
                    self.fonts.draw_centered(&mut r, "VRDoctor", art.x + art.w / 2.0, wb, 30.0, false, [255, 255, 255, 140]);
                }
            }
            if hovered {
                r.stroke_rounded_rect(art.x - 6.0, art.y - 6.0, art.w + 12.0, art.h + 12.0, 18.0, 5.0, accent());
            }
            // Badge pill, bottom-left of the art.
            let (btext, bcol) = badge_of(&g.vr);
            let bw = self.fonts.measure(btext, 22.0, true) + 26.0;
            let bx = art.x + 12.0;
            let by = art.y + art.h - 12.0 - 34.0;
            r.fill_rounded_rect(bx, by, bw, 34.0, 17.0, bcol);
            let bb = by + 6.0 + self.fonts.ascent(22.0, true);
            self.fonts.draw_centered(&mut r, btext, bx + bw / 2.0, bb, 22.0, true, WHITE);
            // Name under the art.
            let name = self.fonts.truncate(&g.name, 29.0, false, cell.w - 10.0);
            let nb = art.y + art.h + 10.0 + self.fonts.ascent(29.0, false);
            self.fonts.draw_centered(&mut r, &name, cell.x + cell.w / 2.0, nb, 29.0, false, WHITE);
            self.regions.push((cell, region));
        }

        // Footer: paging + status.
        let pages = self.pages();
        if pages > 1 {
            if self.page > 0 {
                let p = prev_rect();
                r.fill_rounded_rect(p.x, p.y, p.w, p.h, 14.0,
                    if hover.contains(&Some(Region::PagePrev)) { [51, 65, 85, 255] } else { [30, 41, 59, 255] });
                let pb = p.y + 12.0 + self.fonts.ascent(28.0, true);
                self.fonts.draw_centered(&mut r, "◀", p.x + p.w / 2.0, pb, 28.0, true, WHITE);
                self.regions.push((p, Region::PagePrev));
            }
            if self.page + 1 < pages {
                let n = next_rect();
                r.fill_rounded_rect(n.x, n.y, n.w, n.h, 14.0,
                    if hover.contains(&Some(Region::PageNext)) { [51, 65, 85, 255] } else { [30, 41, 59, 255] });
                let nb2 = n.y + 12.0 + self.fonts.ascent(28.0, true);
                self.fonts.draw_centered(&mut r, "▶", n.x + n.w / 2.0, nb2, 28.0, true, WHITE);
                self.regions.push((n, Region::PageNext));
            }
        }
        let status = if !self.status.is_empty() {
            self.status.clone()
        } else if pages > 1 {
            format!("Page {} of {} — point at a game and pull the trigger", self.page + 1, pages)
        } else {
            "Point at a game and pull the trigger".to_string()
        };
        let scol: Rgba = if self.status_is_error { [248, 113, 113, 255] } else { [203, 213, 225, 255] };
        let sb = 1140.0 + self.fonts.ascent(27.0, false);
        self.fonts.draw_centered(&mut r, &status, PANEL_W as f32 / 2.0, sb, 27.0, false, scol);

        // Laser dots last, on top of everything.
        for dot in dots.iter().flatten() {
            r.fill_circle(dot.0, dot.1, 10.0, accent());
            r.fill_circle(dot.0, dot.1, 4.5, WHITE);
        }
        r
    }

    fn region_at(&self, x: f32, y: f32) -> Option<Region> {
        // Later regions were pushed later; tiles don't overlap buttons.
        self.regions.iter().rev().find(|(rect, _)| rect.contains(x, y)).map(|(_, reg)| *reg)
    }
}

// ---------------------------------------------------------------- math

type Mat4 = [f32; 16]; // column-major

fn mat4_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

fn mat4_from_pose(p: openxr::Posef) -> Mat4 {
    let q = p.orientation;
    let (x, y, z, w) = (q.x, q.y, q.z, q.w);
    let pos = p.position;
    [
        1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w), 0.0,
        2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w), 0.0,
        2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y), 0.0,
        pos.x, pos.y, pos.z, 1.0,
    ]
}

/// Inverse of a rigid (rotation + translation) matrix.
fn mat4_rigid_inverse(m: &Mat4) -> Mat4 {
    let mut out = [0.0f32; 16];
    // Transpose the rotation.
    for col in 0..3 {
        for row in 0..3 {
            out[col * 4 + row] = m[row * 4 + col];
        }
    }
    let (tx, ty, tz) = (m[12], m[13], m[14]);
    out[12] = -(out[0] * tx + out[4] * ty + out[8] * tz);
    out[13] = -(out[1] * tx + out[5] * ty + out[9] * tz);
    out[14] = -(out[2] * tx + out[6] * ty + out[10] * tz);
    out[15] = 1.0;
    out
}

/// OpenGL projection matrix from OpenXR field-of-view tangents.
fn mat4_projection(fov: openxr::Fovf, near: f32, far: f32) -> Mat4 {
    let tan_l = fov.angle_left.tan();
    let tan_r = fov.angle_right.tan();
    let tan_d = fov.angle_down.tan();
    let tan_u = fov.angle_up.tan();
    let sx = 2.0 / (tan_r - tan_l);
    let ox = (tan_r + tan_l) / (tan_r - tan_l);
    let sy = 2.0 / (tan_u - tan_d);
    let oy = (tan_u + tan_d) / (tan_u - tan_d);
    let sz = -(far + near) / (far - near);
    let oz = -(2.0 * far * near) / (far - near);
    [
        sx, 0.0, 0.0, 0.0,
        0.0, sy, 0.0, 0.0,
        ox, oy, sz, -1.0,
        0.0, 0.0, oz, 0.0,
    ]
}

fn quat_rotate(q: openxr::Quaternionf, v: [f32; 3]) -> [f32; 3] {
    let u = [q.x, q.y, q.z];
    let uv = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let uuv = [
        u[1] * uv[2] - u[2] * uv[1],
        u[2] * uv[0] - u[0] * uv[2],
        u[0] * uv[1] - u[1] * uv[0],
    ];
    [
        v[0] + 2.0 * (uv[0] * q.w + uuv[0]),
        v[1] + 2.0 * (uv[1] * q.w + uuv[1]),
        v[2] + 2.0 * (uv[2] * q.w + uuv[2]),
    ]
}

fn v3_sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn v3_add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
fn v3_scale(a: [f32; 3], s: f32) -> [f32; 3] { [a[0] * s, a[1] * s, a[2] * s] }
fn v3_dot(a: [f32; 3], b: [f32; 3]) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn v3_norm(a: [f32; 3]) -> [f32; 3] {
    let l = v3_dot(a, a).sqrt();
    if l > 1e-8 { v3_scale(a, 1.0 / l) } else { a }
}

// ---------------------------------------------------------------- EGL

struct Egl {
    instance: khronos_egl::DynamicInstance<khronos_egl::EGL1_5>,
    display: khronos_egl::Display,
    config: khronos_egl::Config,
    context: khronos_egl::Context,
}

/// Load EGL 1.5. `load_required()` only tries the development name
/// `libEGL.so`; runtime-only installs (Fedora mesa-libEGL, Debian
/// libegl1) ship just `libEGL.so.1`, so fall back to that.
fn load_egl() -> VResult<khronos_egl::DynamicInstance<khronos_egl::EGL1_5>> {
    use khronos_egl as egl;
    type Dyn = egl::DynamicInstance<egl::EGL1_5>;
    if let Ok(i) = unsafe { Dyn::load_required() } { return Ok(i); }
    for cand in [
        "libEGL.so.1",
        "/usr/lib64/libEGL.so.1",
        "/usr/lib/libEGL.so.1",
        "/usr/lib/x86_64-linux-gnu/libEGL.so.1",
    ] {
        if let Ok(lib) = unsafe { libloading::Library::new(cand) } {
            if let Ok(i) = unsafe { Dyn::load_required_from(lib) } { return Ok(i); }
        }
    }
    Err("could not load libEGL (tried libEGL.so and .so.1) — is Mesa/libegl installed?".into())
}

impl Egl {
    fn new() -> VResult<Egl> {
        use khronos_egl as egl;
        let instance = load_egl()?;
        // Prefer the surfaceless platform: no X11/Wayland connection needed,
        // which keeps us compositor-agnostic. Fall back to the default display.
        const PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31DD;
        let display = unsafe { instance.get_platform_display(PLATFORM_SURFACELESS_MESA, std::ptr::null_mut(), &[egl::NONE as usize]) }
            .ok()
            .or_else(|| unsafe { instance.get_display(egl::DEFAULT_DISPLAY) })
            .ok_or("EGL: no display (surfaceless platform and default display both failed)")?;
        instance.initialize(display).map_err(|e| format!("EGL: initialize failed ({e:?})"))?;
        instance.bind_api(egl::OPENGL_API).map_err(|e| format!("EGL: bind OpenGL API failed ({e:?})"))?;
        let config_attribs = [
            egl::SURFACE_TYPE, egl::PBUFFER_BIT,
            egl::RENDERABLE_TYPE, egl::OPENGL_BIT,
            egl::RED_SIZE, 8, egl::GREEN_SIZE, 8, egl::BLUE_SIZE, 8, egl::ALPHA_SIZE, 8,
            egl::DEPTH_SIZE, 0, egl::STENCIL_SIZE, 0,
            egl::NONE,
        ];
        // NB: capacity must be > 0 — khronos-egl 6.0's choose_config
        // silently no-ops on a zero-capacity Vec.
        let mut configs = Vec::with_capacity(64);
        instance.choose_config(display, &config_attribs, &mut configs)
            .map_err(|e| format!("EGL: choose_config failed ({e:?})"))?;
        if configs.is_empty() {
            instance.choose_config(display, &[egl::RENDERABLE_TYPE, egl::OPENGL_BIT, egl::NONE], &mut configs)
                .map_err(|e| format!("EGL: choose_config (fallback) failed ({e:?})"))?;
        }
        let config = *configs.first().ok_or("EGL: no suitable config")?;
        let context = instance.create_context(display, config, None, &[
            egl::CONTEXT_MAJOR_VERSION, 3,
            egl::CONTEXT_MINOR_VERSION, 3,
            egl::CONTEXT_OPENGL_PROFILE_MASK, egl::CONTEXT_OPENGL_CORE_PROFILE_BIT,
            egl::NONE,
        ]).map_err(|e| format!("EGL: create_context failed ({e:?})"))?;
        let egl = Egl { instance, display, config, context };
        egl.make_current()?;
        Ok(egl)
    }

    fn make_current(&self) -> VResult<()> {
        self.instance.make_current(self.display, None, None, Some(self.context))
            .map_err(|e| format!("EGL: make_current failed ({e:?})"))
    }

    /// The address of eglGetProcAddress itself, in the exact shape
    /// XrGraphicsBindingEGLMNDX wants it.
    fn get_proc_address_fn(&self) -> openxr::sys::pfn::EglGetProcAddressMNDX {
        let f = self.instance.get_proc_address("eglGetProcAddress")
            .expect("eglGetProcAddress must exist");
        unsafe { std::mem::transmute(f) }
    }
}

impl Drop for Egl {
    fn drop(&mut self) {
        let _ = self.instance.make_current(self.display, None, None, None);
        let _ = self.instance.destroy_context(self.display, self.context);
        let _ = self.instance.terminate(self.display);
    }
}

// ---------------------------------------------------------------- GL scene

const SPHERE_VERT: &str = r#"#version 330 core
layout(location = 0) in vec3 a_pos;
uniform mat4 u_vp;
out vec3 v_dir;
void main() {
    v_dir = a_pos;
    gl_Position = u_vp * vec4(a_pos, 1.0);
}
"#;

const SPHERE_FRAG: &str = r#"#version 330 core
in vec3 v_dir;
uniform sampler2D u_tex;
uniform int u_has_tex;
out vec4 o_color;
void main() {
    vec3 d = normalize(v_dir);
    if (u_has_tex == 1) {
        float u = atan(d.x, -d.z) / 6.28318530718 + 0.5;
        float v = acos(clamp(d.y, -1.0, 1.0)) / 3.14159265359;
        o_color = texture(u_tex, vec2(u, v));
    } else {
        vec3 horizon = vec3(0.115, 0.145, 0.215);
        vec3 zenith = vec3(0.030, 0.043, 0.082);
        vec3 ground = vec3(0.045, 0.050, 0.070);
        vec3 c = d.y >= 0.0
            ? mix(horizon, zenith, pow(clamp(d.y, 0.0, 1.0), 0.6))
            : mix(horizon, ground, clamp(-d.y * 2.2, 0.0, 1.0));
        o_color = vec4(c, 1.0);
    }
}
"#;

const LINE_VERT: &str = r#"#version 330 core
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec4 a_color;
uniform mat4 u_vp;
out vec4 v_color;
void main() {
    v_color = a_color;
    gl_Position = u_vp * vec4(a_pos, 1.0);
}
"#;

const LINE_FRAG: &str = r#"#version 330 core
in vec4 v_color;
out vec4 o_color;
void main() { o_color = v_color; }
"#;

fn compile_program(gl: &glow::Context, vs_src: &str, fs_src: &str) -> VResult<glow::Program> {
    use glow::HasContext;
    unsafe {
        let program = gl.create_program().map_err(|e| format!("GL: create_program: {e}"))?;
        let mut shaders = vec![];
        for (kind, src) in [(glow::VERTEX_SHADER, vs_src), (glow::FRAGMENT_SHADER, fs_src)] {
            let sh = gl.create_shader(kind).map_err(|e| format!("GL: create_shader: {e}"))?;
            gl.shader_source(sh, src);
            gl.compile_shader(sh);
            if !gl.get_shader_compile_status(sh) {
                return Err(format!("GL: shader compile: {}", gl.get_shader_info_log(sh)));
            }
            gl.attach_shader(program, sh);
            shaders.push(sh);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            return Err(format!("GL: program link: {}", gl.get_program_info_log(program)));
        }
        for sh in shaders { gl.delete_shader(sh); }
        Ok(program)
    }
}

struct GlScene {
    gl: glow::Context,
    sphere_program: glow::Program,
    sphere_vao: glow::VertexArray,
    sphere_indices: i32,
    line_program: glow::Program,
    line_vao: glow::VertexArray,
    line_vbo: glow::Buffer,
    pano_texture: Option<glow::Texture>,
    fbos: HashMap<u32, glow::Framebuffer>, // per swapchain texture
}

impl GlScene {
    fn new(egl: &Egl) -> VResult<GlScene> {
        use glow::HasContext;
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.instance.get_proc_address(name)
                    .map(|f| f as *const std::ffi::c_void)
                    .unwrap_or(std::ptr::null())
            })
        };
        let sphere_program = compile_program(&gl, SPHERE_VERT, SPHERE_FRAG)?;
        let line_program = compile_program(&gl, LINE_VERT, LINE_FRAG)?;

        // Inside-out sphere (positions only), radius 30.
        let (verts, indices) = sphere_geometry(30.0, 64, 32);
        let sphere_vao;
        unsafe {
            sphere_vao = gl.create_vertex_array().map_err(|e| format!("GL: vao: {e}"))?;
            gl.bind_vertex_array(Some(sphere_vao));
            let vbo = gl.create_buffer().map_err(|e| format!("GL: vbo: {e}"))?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck_f32(&verts), glow::STATIC_DRAW);
            let ebo = gl.create_buffer().map_err(|e| format!("GL: ebo: {e}"))?;
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytemuck_u32(&indices), glow::STATIC_DRAW);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 12, 0);
            gl.enable_vertex_attrib_array(0);
            gl.bind_vertex_array(None);
        }
        // Dynamic laser buffer: 2 lines × 2 verts × (pos3 + color4).
        let (line_vao, line_vbo);
        unsafe {
            line_vao = gl.create_vertex_array().map_err(|e| format!("GL: line vao: {e}"))?;
            gl.bind_vertex_array(Some(line_vao));
            line_vbo = gl.create_buffer().map_err(|e| format!("GL: line vbo: {e}"))?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(line_vbo));
            gl.buffer_data_size(glow::ARRAY_BUFFER, 2 * 2 * 7 * 4, glow::DYNAMIC_DRAW);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 28, 0);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(1, 4, glow::FLOAT, false, 28, 12);
            gl.enable_vertex_attrib_array(1);
            gl.bind_vertex_array(None);
        }
        // Panorama texture (optional — gradient fallback in the shader).
        let pano_texture = find_panorama().and_then(|path| {
            let img = image::open(&path).ok()?.to_rgba8();
            let (w, h) = img.dimensions();
            let (img, w, h) = if w > 4096 {
                let nh = (h as f32 * 4096.0 / w as f32) as u32;
                (image::imageops::resize(&img, 4096, nh, image::imageops::FilterType::Triangle), 4096, nh)
            } else { (img, w, h) };
            unsafe {
                let tex = gl.create_texture().ok()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::SRGB8_ALPHA8 as i32, w as i32, h as i32, 0,
                                glow::RGBA, glow::UNSIGNED_BYTE, Some(img.as_raw().as_slice()));
                gl.generate_mipmap(glow::TEXTURE_2D);
                gl.bind_texture(glow::TEXTURE_2D, None);
                Some(tex)
            }
        });

        Ok(GlScene {
            gl,
            sphere_program,
            sphere_vao,
            sphere_indices: indices.len() as i32,
            line_program,
            line_vao,
            line_vbo,
            pano_texture,
            fbos: HashMap::new(),
        })
    }

    fn framebuffer_for(&mut self, tex: u32) -> VResult<glow::Framebuffer> {
        use glow::HasContext;
        if let Some(f) = self.fbos.get(&tex) { return Ok(*f); }
        unsafe {
            let fbo = self.gl.create_framebuffer().map_err(|e| format!("GL: fbo: {e}"))?;
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            self.gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0,
                                           glow::TEXTURE_2D, Some(native_texture(tex)), 0);
            let status = self.gl.check_framebuffer_status(glow::FRAMEBUFFER);
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if status != glow::FRAMEBUFFER_COMPLETE {
                return Err(format!("GL: framebuffer incomplete (status {status:#x})"));
            }
            self.fbos.insert(tex, fbo);
            Ok(fbo)
        }
    }

    /// Render the panorama + laser beams into one eye's swapchain texture.
    /// `lasers` holds up to 2 (origin, end) beam segments in space coords.
    fn render_eye(&mut self, tex: u32, w: u32, h: u32, view: openxr::View, lasers: &[[[f32; 3]; 2]]) -> VResult<()> {
        use glow::HasContext;
        let fbo = self.framebuffer_for(tex)?;
        let proj = mat4_projection(view.fov, 0.05, 100.0);
        let view_mat = mat4_rigid_inverse(&mat4_from_pose(view.pose));
        let mut sky_view = view_mat;
        sky_view[12] = 0.0;
        sky_view[13] = 0.0;
        sky_view[14] = 0.0;
        let sky_vp = mat4_mul(&proj, &sky_view);
        let vp = mat4_mul(&proj, &view_mat);
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            self.gl.viewport(0, 0, w as i32, h as i32);
            self.gl.disable(glow::DEPTH_TEST);
            self.gl.clear_color(0.03, 0.04, 0.08, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
            // Panorama sphere.
            self.gl.use_program(Some(self.sphere_program));
            if let Some(loc) = self.gl.get_uniform_location(self.sphere_program, "u_vp") {
                self.gl.uniform_matrix_4_f32_slice(Some(&loc), false, &sky_vp);
            }
            if let Some(loc) = self.gl.get_uniform_location(self.sphere_program, "u_has_tex") {
                self.gl.uniform_1_i32(Some(&loc), if self.pano_texture.is_some() { 1 } else { 0 });
            }
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.bind_texture(glow::TEXTURE_2D, self.pano_texture);
            if let Some(loc) = self.gl.get_uniform_location(self.sphere_program, "u_tex") {
                self.gl.uniform_1_i32(Some(&loc), 0);
            }
            self.gl.bind_vertex_array(Some(self.sphere_vao));
            self.gl.draw_elements(glow::TRIANGLES, self.sphere_indices, glow::UNSIGNED_INT, 0);
            self.gl.bind_vertex_array(None);
            // Laser beams.
            if !lasers.is_empty() {
                let mut data: Vec<f32> = Vec::with_capacity(lasers.len() * 14);
                let a = accent();
                let (r, g, b) = (a[0] as f32 / 255.0, a[1] as f32 / 255.0, a[2] as f32 / 255.0);
                for [origin, end] in lasers {
                    data.extend_from_slice(origin);
                    data.extend_from_slice(&[r, g, b, 0.9]); // accent, bright at hand
                    data.extend_from_slice(end);
                    data.extend_from_slice(&[r, g, b, 0.15]); // fading out
                }
                self.gl.bind_vertex_array(Some(self.line_vao));
                self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.line_vbo));
                self.gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytemuck_f32(&data));
                self.gl.use_program(Some(self.line_program));
                if let Some(loc) = self.gl.get_uniform_location(self.line_program, "u_vp") {
                    self.gl.uniform_matrix_4_f32_slice(Some(&loc), false, &vp);
                }
                self.gl.enable(glow::BLEND);
                self.gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                self.gl.draw_arrays(glow::LINES, 0, (lasers.len() * 2) as i32);
                self.gl.disable(glow::BLEND);
                self.gl.bind_vertex_array(None);
            }
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        Ok(())
    }
}

/// glow identifies textures by its own handle type (a NonZeroU32);
/// swapchain images are raw GL texture names. Wrap the name.
fn native_texture(name: u32) -> glow::Texture {
    glow::NativeTexture(std::num::NonZeroU32::new(name).expect("swapchain texture name is nonzero"))
}

fn bytemuck_f32(v: &[f32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}
fn bytemuck_u32(v: &[u32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

fn sphere_geometry(radius: f32, sectors: usize, rings: usize) -> (Vec<f32>, Vec<u32>) {
    let mut verts = Vec::with_capacity((sectors + 1) * (rings + 1) * 3);
    let mut indices = Vec::with_capacity(sectors * rings * 6);
    for r in 0..=rings {
        let phi = std::f32::consts::PI * r as f32 / rings as f32; // 0 at +Y
        for s in 0..=sectors {
            let theta = 2.0 * std::f32::consts::PI * s as f32 / sectors as f32;
            let x = radius * phi.sin() * theta.sin();
            let y = radius * phi.cos();
            let z = -radius * phi.sin() * theta.cos();
            verts.extend_from_slice(&[x, y, z]);
        }
    }
    for r in 0..rings {
        for s in 0..sectors {
            let a = (r * (sectors + 1) + s) as u32;
            let b = a + 1;
            let c = a + (sectors + 1) as u32;
            let d = c + 1;
            // Inside-out: viewed from the centre, so wind backwards.
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    (verts, indices)
}

// ---------------------------------------------------------------- GL uploads

impl GlScene {
    fn prepare_texture(&self, tex: u32) {
        use glow::HasContext;
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(native_texture(tex)));
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            self.gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }

    /// Upload the CPU panel raster into a quad-layer swapchain texture.
    fn upload_panel(&self, tex: u32, raster: &Raster) {
        use glow::HasContext;
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(native_texture(tex)));
            self.gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, raster.w as i32, raster.h as i32,
                                     glow::RGBA, glow::UNSIGNED_BYTE,
                                     glow::PixelUnpackData::Slice(&raster.px));
            self.gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }
}

// ---------------------------------------------------------------- interaction

const PANEL_WORLD_W: f32 = 2.3;
const PANEL_WORLD_H: f32 = PANEL_WORLD_W * PANEL_H as f32 / PANEL_W as f32; // 1.4375

/// Place the panel 2.1 m in front of where the user first looks,
/// yaw-locked so it stays put in the room.
fn compute_panel_pose(view: openxr::Posef) -> openxr::Posef {
    let fwd3 = quat_rotate(view.orientation, [0.0, 0.0, -1.0]);
    let mut fwd = [fwd3[0], 0.0, fwd3[2]];
    if v3_dot(fwd, fwd) < 1e-6 { fwd = [0.0, 0.0, -1.0]; }
    let fwd = v3_norm(fwd);
    let theta = (-fwd[0]).atan2(-fwd[2]);
    let half = theta / 2.0;
    openxr::Posef {
        orientation: openxr::Quaternionf { x: 0.0, y: half.sin(), z: 0.0, w: half.cos() },
        position: openxr::Vector3f {
            x: view.position.x + fwd[0] * 2.1,
            y: view.position.y - 0.12,
            z: view.position.z + fwd[2] * 2.1,
        },
    }
}

/// Intersect a controller ray with the panel quad.
/// Returns (raster_x, raster_y, distance) on a hit.
fn intersect_panel(origin: [f32; 3], dir: [f32; 3], pose: openxr::Posef) -> Option<(f32, f32, f32)> {
    let q = pose.orientation;
    let normal = quat_rotate(q, [0.0, 0.0, 1.0]);
    let right = quat_rotate(q, [1.0, 0.0, 0.0]);
    let up = quat_rotate(q, [0.0, 1.0, 0.0]);
    let center = [pose.position.x, pose.position.y, pose.position.z];
    let denom = v3_dot(dir, normal);
    if denom.abs() < 1e-6 { return None; }
    let t = v3_dot(v3_sub(center, origin), normal) / denom;
    if t <= 0.0 { return None; }
    let p = v3_add(origin, v3_scale(dir, t));
    let rel = v3_sub(p, center);
    let dx = v3_dot(rel, right);
    let dy = v3_dot(rel, up);
    if dx.abs() > PANEL_WORLD_W / 2.0 || dy.abs() > PANEL_WORLD_H / 2.0 { return None; }
    let rx = (dx / PANEL_WORLD_W + 0.5) * PANEL_W as f32;
    let ry = (0.5 - dy / PANEL_WORLD_H) * PANEL_H as f32;
    Some((rx, ry, t))
}

fn activate(region: Region, panel: &mut Panel, exit_at: &mut Option<Instant>) {
    match region {
        Region::Quit => *exit_at = Some(Instant::now()),
        Region::PagePrev => panel.page = panel.page.saturating_sub(1),
        Region::PageNext => panel.page = (panel.page + 1).min(panel.pages() - 1),
        Region::Launch(idx) => {
            let g = &panel.games[idx];
            let target = if g.kind == "steam" { g.appid.to_string() } else { g.name.clone() };
            let spawned = Command::new("setsid")
                .arg(sibling("vrdoctor"))
                .args(["launch", "--game", &target])
                .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
                .spawn();
            match spawned {
                Ok(_) => {
                    panel.status = format!("Launching {}…", g.name);
                    panel.status_is_error = false;
                    *exit_at = Some(Instant::now() + Duration::from_millis(2000));
                }
                Err(e) => {
                    panel.status = format!("Launch failed: {e}");
                    panel.status_is_error = true;
                }
            }
        }
    }
}

// ---------------------------------------------------------------- XR session

fn fmt_version(v: openxr::Version) -> String { format!("{}.{}.{}", v.major(), v.minor(), v.patch()) }

fn create_egl_session(
    instance: &openxr::Instance,
    system: openxr::SystemId,
    egl: &Egl,
) -> VResult<(openxr::Session<openxr::OpenGL>, openxr::FrameWaiter, openxr::FrameStream<openxr::OpenGL>)> {
    use openxr::sys;
    let binding = sys::GraphicsBindingEGLMNDX {
        ty: sys::GraphicsBindingEGLMNDX::TYPE,
        next: std::ptr::null(),
        get_proc_address: Some(egl.get_proc_address_fn()),
        display: egl.display.as_ptr(),
        config: egl.config.as_ptr(),
        context: egl.context.as_ptr(),
    };
    let info = sys::SessionCreateInfo {
        ty: sys::SessionCreateInfo::TYPE,
        next: &binding as *const _ as *const std::ffi::c_void,
        create_flags: Default::default(),
        system_id: system,
    };
    let mut raw = sys::Session::NULL;
    let res = unsafe { (instance.fp().create_session)(instance.as_raw(), &info, &mut raw) };
    if res.into_raw() < 0 {
        return Err(format!("xrCreateSession with the EGL binding failed (XrResult {})", res.into_raw()));
    }
    Ok(unsafe { openxr::Session::from_raw(instance.clone(), raw, Box::new(())) })
}

struct EyeSwap {
    swapchain: openxr::Swapchain<openxr::OpenGL>,
    images: Vec<u32>,
    w: u32,
    h: u32,
}

struct Opts {
    probe: bool,
    frames: Option<u64>,
}

/// Load the OpenXR entry points. `Entry::load()` only tries the
/// development name `libopenxr_loader.so`; runtime packages on several
/// distros (Fedora's openxr-libs, Debian's libopenxr-loader1) ship just
/// the versioned `libopenxr_loader.so.1`, so fall back to that (a bare
/// soname is searched in the ldconfig cache) and to multiarch paths.
fn load_entry() -> VResult<openxr::Entry> {
    if let Ok(e) = unsafe { openxr::Entry::load() } { return Ok(e); }
    for cand in [
        "libopenxr_loader.so.1",
        "/usr/lib64/libopenxr_loader.so.1",
        "/usr/lib/libopenxr_loader.so.1",
        "/usr/lib/x86_64-linux-gnu/libopenxr_loader.so.1",
    ] {
        if let Ok(e) = unsafe { openxr::Entry::load_from(std::path::Path::new(cand)) } { return Ok(e); }
    }
    Err("no OpenXR loader found (tried libopenxr_loader.so and .so.1). vrdoctor-vr needs the \
         OpenXR loader — Arch package `openxr`, Fedora `openxr-libs` — and a running runtime \
         such as WiVRn.".into())
}

fn run(opts: &Opts) -> VResult<()> {
    use openxr::{EnvironmentBlendMode, ViewConfigurationType};
    let entry = load_entry()?;
    let available = entry.enumerate_extensions().map_err(|e| format!("enumerate extensions: {e:?}"))?;
    if !available.khr_opengl_enable || !available.mndx_egl_enable {
        return Err("the active OpenXR runtime does not offer XR_KHR_opengl_enable + \
                    XR_MNDX_egl_enable, which vrdoctor-vr needs for its EGL/OpenGL path".into());
    }
    let mut exts = openxr::ExtensionSet::default();
    exts.khr_opengl_enable = true;
    exts.mndx_egl_enable = true;
    let instance = entry.create_instance(
        &openxr::ApplicationInfo {
            application_name: "VRDoctor VR",
            application_version: 1,
            engine_name: "VRDoctor",
            engine_version: 1,
            api_version: openxr::Version::new(1, 0, 0),
        },
        &exts,
        &[],
    ).map_err(|e| format!("xrCreateInstance: {e:?}"))?;
    let props = instance.properties().map_err(|e| format!("instance properties: {e:?}"))?;
    let system = instance.system(openxr::FormFactor::HEAD_MOUNTED_DISPLAY)
        .map_err(|e| format!("no HMD system: {e:?}"))?;
    let sysprops = instance.system_properties(system).map_err(|e| format!("system properties: {e:?}"))?;
    if opts.probe {
        println!("runtime: {} {}", props.runtime_name, fmt_version(props.runtime_version));
        println!("system: {}", sysprops.system_name);
        println!("extensions: XR_KHR_opengl_enable + XR_MNDX_egl_enable available");
        return Ok(());
    }

    // EGL + GL (context stays current on this thread for the whole run).
    let egl = Egl::new()?;
    let _reqs = <openxr::OpenGL as openxr::Graphics>::requirements(&instance, system)
        .map_err(|e| format!("OpenGL graphics requirements: {e:?}"))?;
    let (session, mut waiter, mut stream) = create_egl_session(&instance, system, &egl)?;
    let mut scene = GlScene::new(&egl)?;

    let local_space = session.create_reference_space(openxr::ReferenceSpaceType::LOCAL, openxr::Posef::IDENTITY)
        .map_err(|e| format!("create LOCAL reference space: {e:?}"))?;
    let view_configs = instance.enumerate_view_configuration_views(system, ViewConfigurationType::PRIMARY_STEREO)
        .map_err(|e| format!("view configuration views: {e:?}"))?;
    let blend = instance.enumerate_environment_blend_modes(system, ViewConfigurationType::PRIMARY_STEREO)
        .map(|v| v.first().copied().unwrap_or(EnvironmentBlendMode::OPAQUE))
        .unwrap_or(EnvironmentBlendMode::OPAQUE);
    let formats = session.enumerate_swapchain_formats().map_err(|e| format!("swapchain formats: {e:?}"))?;
    let color_format = [glow::SRGB8_ALPHA8, glow::RGBA8].iter()
        .find(|f| formats.contains(f)).copied()
        .or_else(|| formats.first().copied())
        .ok_or("runtime offers no swapchain formats")?;

    // Eye swapchains.
    let mut eyes: Vec<EyeSwap> = vec![];
    for vc in &view_configs {
        let (w, h) = (vc.recommended_image_rect_width.max(16), vc.recommended_image_rect_height.max(16));
        let sc = session.create_swapchain(&openxr::SwapchainCreateInfo {
            create_flags: openxr::SwapchainCreateFlags::EMPTY,
            usage_flags: openxr::SwapchainUsageFlags::COLOR_ATTACHMENT | openxr::SwapchainUsageFlags::SAMPLED,
            format: color_format,
            sample_count: 1,
            width: w,
            height: h,
            face_count: 1,
            array_size: 1,
            mip_count: 1,
        }).map_err(|e| format!("create eye swapchain: {e:?}"))?;
        let images = sc.enumerate_images().map_err(|e| format!("enumerate eye images: {e:?}"))?;
        for img in &images { scene.prepare_texture(*img); }
        eyes.push(EyeSwap { swapchain: sc, images, w, h });
    }
    // Panel (quad layer) swapchain.
    let mut quad_swapchain = session.create_swapchain(&openxr::SwapchainCreateInfo {
        create_flags: openxr::SwapchainCreateFlags::EMPTY,
        usage_flags: openxr::SwapchainUsageFlags::SAMPLED,
        format: color_format,
        sample_count: 1,
        width: PANEL_W as u32,
        height: PANEL_H as u32,
        face_count: 1,
        array_size: 1,
        mip_count: 1,
    }).map_err(|e| format!("create panel swapchain: {e:?}"))?;
    let quad_images = quad_swapchain.enumerate_images().map_err(|e| format!("enumerate panel images: {e:?}"))?;
    if let Some(img) = quad_images.first() { scene.prepare_texture(*img); }

    // Actions: trigger click + aim pose on both hands.
    let action_set = instance.create_action_set("vrdoctor", "VRDoctor", 0)
        .map_err(|e| format!("create action set: {e:?}"))?;
    let hand_paths = [
        instance.string_to_path("/user/hand/left").map_err(|e| format!("path: {e:?}"))?,
        instance.string_to_path("/user/hand/right").map_err(|e| format!("path: {e:?}"))?,
    ];
    let click = action_set.create_action::<bool>("click", "Select", &hand_paths)
        .map_err(|e| format!("create click action: {e:?}"))?;
    let aim = action_set.create_action::<openxr::Posef>("aim", "Aim", &hand_paths)
        .map_err(|e| format!("create aim action: {e:?}"))?;
    for (profile, click_l, click_r) in [
        ("/interaction_profiles/oculus/touch_controller", "/user/hand/left/input/trigger/click", "/user/hand/right/input/trigger/click"),
        ("/interaction_profiles/khr/simple_controller", "/user/hand/left/input/select/click", "/user/hand/right/input/select/click"),
    ] {
        let bindings = [
            openxr::Binding::new(&click, instance.string_to_path(click_l).map_err(|e| format!("path: {e:?}"))?),
            openxr::Binding::new(&click, instance.string_to_path(click_r).map_err(|e| format!("path: {e:?}"))?),
            openxr::Binding::new(&aim, instance.string_to_path("/user/hand/left/input/aim/pose").map_err(|e| format!("path: {e:?}"))?),
            openxr::Binding::new(&aim, instance.string_to_path("/user/hand/right/input/aim/pose").map_err(|e| format!("path: {e:?}"))?),
        ];
        // Best-effort: a runtime that rejects one profile's suggestions
        // (e.g. very old Monado builds) must not stop the app from
        // rendering — it just falls back to its own default bindings.
        if let Err(e) = instance.suggest_interaction_profile_bindings(
            instance.string_to_path(profile).map_err(|e| format!("path: {e:?}"))?, &bindings)
        {
            eprintln!("vrdoctor-vr: warning: bindings for {profile} rejected ({e:?}); continuing");
        }
    }
    session.attach_action_sets(&[&action_set]).map_err(|e| format!("attach action sets: {e:?}"))?;
    let aim_spaces = [
        aim.create_space(session.clone(), hand_paths[0], openxr::Posef::IDENTITY).map_err(|e| format!("aim space: {e:?}"))?,
        aim.create_space(session.clone(), hand_paths[1], openxr::Posef::IDENTITY).map_err(|e| format!("aim space: {e:?}"))?,
    ];

    // Panel state.
    let mut panel = Panel::new(load_games())?;
    let mut raster = panel.render([None, None], [None, None]);
    let mut upload_needed = true;
    type Sig = (i64, i64, [Option<(i32, i32)>; 2], usize, String, bool);
    let region_key = |r: Option<Region>| -> i64 {
        match r { None => -1, Some(Region::Quit) => -2, Some(Region::PagePrev) => -3,
                  Some(Region::PageNext) => -4, Some(Region::Launch(i)) => i as i64 }
    };
    let mut last_sig: Option<Sig> = None;

    let mut event_buffer = openxr::EventDataBuffer::new();
    let mut running = false;
    let mut focused = false;
    let mut panel_pose: Option<openxr::Posef> = None;
    let mut exit_at: Option<Instant> = None;
    let mut exit_requested = false;
    let mut exit_deadline: Option<Instant> = None;
    let mut rendered_frames: u64 = 0;
    let started = Instant::now();

    'outer: loop {
        // ---- events
        loop {
            match instance.poll_event(&mut event_buffer) {
                Ok(Some(openxr::Event::SessionStateChanged(e))) => {
                    let state = e.state();
                    if state == openxr::SessionState::READY {
                        session.begin(ViewConfigurationType::PRIMARY_STEREO)
                            .map_err(|err| format!("xrBeginSession: {err:?}"))?;
                        running = true;
                    } else if state == openxr::SessionState::STOPPING {
                        let _ = session.end();
                        running = false;
                    } else if state == openxr::SessionState::EXITING
                        || state == openxr::SessionState::LOSS_PENDING {
                        break 'outer;
                    }
                    focused = state == openxr::SessionState::FOCUSED;
                }
                Ok(Some(openxr::Event::InstanceLossPending(_))) => break 'outer,
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => break,
            }
        }
        // ---- scheduled exit (after a launch, or Quit)
        if !exit_requested {
            if let Some(t) = exit_at {
                if Instant::now() >= t {
                    let _ = session.request_exit();
                    exit_requested = true;
                    exit_deadline = Some(Instant::now() + Duration::from_secs(2));
                }
            }
        } else if exit_deadline.map(|d| Instant::now() >= d).unwrap_or(false) {
            break 'outer; // runtime ignored the exit request; leave anyway
        }
        if opts.frames.is_some() && started.elapsed() > Duration::from_secs(180) {
            return Err("timed out waiting for frames (--frames mode safety net)".into());
        }
        if !running {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        // ---- frame
        let frame_state = waiter.wait().map_err(|e| format!("xrWaitFrame: {e:?}"))?;
        stream.begin().map_err(|e| format!("xrBeginFrame: {e:?}"))?;
        let time = frame_state.predicted_display_time;
        if !frame_state.should_render {
            stream.end(time, blend, &[]).map_err(|e| format!("xrEndFrame: {e:?}"))?;
            continue;
        }
        let (view_flags, views) = match session.locate_views(ViewConfigurationType::PRIMARY_STEREO, time, &local_space) {
            Ok((flags, views)) => (flags, views),
            Err(_) => (openxr::ViewStateFlags::EMPTY, vec![]),
        };
        if panel_pose.is_none()
            && view_flags.contains(openxr::ViewStateFlags::ORIENTATION_VALID)
            && !views.is_empty() {
            panel_pose = Some(compute_panel_pose(views[0].pose));
        }
        // ---- input
        let mut hovers: [Option<Region>; 2] = [None, None];
        let mut dots: [Option<(f32, f32)>; 2] = [None, None];
        let mut lasers: Vec<[[f32; 3]; 2]> = vec![];
        if focused {
            let _ = session.sync_actions(&[openxr::ActiveActionSet::new(&action_set)]);
            for hand in 0..2 {
                let Ok(loc) = aim_spaces[hand].locate(&local_space, time) else { continue };
                if !loc.location_flags.contains(openxr::SpaceLocationFlags::POSITION_VALID)
                    || !loc.location_flags.contains(openxr::SpaceLocationFlags::ORIENTATION_VALID) { continue; }
                let origin = [loc.pose.position.x, loc.pose.position.y, loc.pose.position.z];
                let dir = v3_norm(quat_rotate(loc.pose.orientation, [0.0, 0.0, -1.0]));
                let mut end = v3_add(origin, v3_scale(dir, 3.5));
                if let Some(pp) = panel_pose {
                    if let Some((rx, ry, t)) = intersect_panel(origin, dir, pp) {
                        dots[hand] = Some((rx, ry));
                        hovers[hand] = panel.region_at(rx, ry);
                        end = v3_add(origin, v3_scale(dir, t));
                    }
                }
                lasers.push([origin, end]);
                if let Ok(st) = click.state(&session, hand_paths[hand]) {
                    if st.changed_since_last_sync && st.current_state {
                        if let Some(region) = hovers[hand] {
                            activate(region, &mut panel, &mut exit_at);
                        }
                    }
                }
            }
        }
        // ---- panel re-render if anything visible changed
        let sig: Sig = (
            region_key(hovers[0]),
            region_key(hovers[1]),
            dots.map(|d| d.map(|(x, y)| ((x / 6.0) as i32, (y / 6.0) as i32))),
            panel.page,
            panel.status.clone(),
            panel.status_is_error,
        );
        if last_sig.as_ref() != Some(&sig) {
            raster = panel.render(hovers, dots);
            upload_needed = true;
            last_sig = Some(sig);
        }
        // ---- render eyes
        let mut proj_views = Vec::with_capacity(views.len());
        for (view, eye) in views.iter().zip(eyes.iter_mut()) {
            let idx = eye.swapchain.acquire_image().map_err(|e| format!("acquire eye image: {e:?}"))?;
            eye.swapchain.wait_image(openxr::Duration::INFINITE).map_err(|e| format!("wait eye image: {e:?}"))?;
            let tex = eye.images[idx as usize];
            scene.render_eye(tex, eye.w, eye.h, *view, &lasers)?;
            eye.swapchain.release_image().map_err(|e| format!("release eye image: {e:?}"))?;
            proj_views.push(
                openxr::CompositionLayerProjectionView::new()
                    .pose(view.pose)
                    .fov(view.fov)
                    .sub_image(openxr::SwapchainSubImage::new()
                        .swapchain(&eye.swapchain)
                        .image_rect(openxr::Rect2Di {
                            offset: openxr::Offset2Di { x: 0, y: 0 },
                            extent: openxr::Extent2Di { width: eye.w as i32, height: eye.h as i32 },
                        })
                        .image_array_index(0)),
            );
        }
        // ---- panel texture upload
        if upload_needed {
            let idx = quad_swapchain.acquire_image().map_err(|e| format!("acquire panel image: {e:?}"))?;
            quad_swapchain.wait_image(openxr::Duration::INFINITE).map_err(|e| format!("wait panel image: {e:?}"))?;
            let tex = quad_images[idx as usize];
            scene.upload_panel(tex, &raster);
            quad_swapchain.release_image().map_err(|e| format!("release panel image: {e:?}"))?;
            upload_needed = false;
        }
        // ---- submit
        let proj = openxr::CompositionLayerProjection::new().space(&local_space).views(&proj_views);
        let mut layers: Vec<&openxr::CompositionLayerBase<'_, openxr::OpenGL>> = vec![&*proj];
        let quad;
        if let Some(pp) = panel_pose {
            quad = openxr::CompositionLayerQuad::new()
                .space(&local_space)
                .eye_visibility(openxr::EyeVisibility::BOTH)
                .sub_image(openxr::SwapchainSubImage::new()
                    .swapchain(&quad_swapchain)
                    .image_rect(openxr::Rect2Di {
                        offset: openxr::Offset2Di { x: 0, y: 0 },
                        extent: openxr::Extent2Di { width: PANEL_W as i32, height: PANEL_H as i32 },
                    })
                    .image_array_index(0))
                .pose(pp)
                .size(openxr::Extent2Df { width: PANEL_WORLD_W, height: PANEL_WORLD_H });
            layers.push(&*quad);
        }
        stream.end(time, blend, &layers).map_err(|e| format!("xrEndFrame: {e:?}"))?;
        rendered_frames += 1;
        if let Some(limit) = opts.frames {
            if rendered_frames >= limit {
                println!("vrdoctor-vr: rendered {rendered_frames} frames");
                let _ = session.request_exit();
                break 'outer;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- test hooks

fn dump_panel(path: &str) -> VResult<()> {
    let mut games = load_games();
    if games.is_empty() {
        games = vec![
            VrGame { kind: "steam".into(), appid: 450390, name: "The Lab".into(), vr: "vr".into() },
            VrGame { kind: "steam".into(), appid: 275850, name: "No Man's Sky".into(), vr: "vr2d".into() },
            VrGame { kind: "steam".into(), appid: 730, name: "Counter-Strike 2".into(), vr: "2d".into() },
            VrGame { kind: "custom".into(), appid: 0, name: "Homebrew VR Port".into(), vr: "vr".into() },
        ];
    }
    let mut panel = Panel::new(games)?;
    let slot = if panel.games.len() > 1 { 1 } else { 0 };
    let art = art_rect(slot);
    let dot = (art.x + art.w / 2.0, art.y + art.h / 2.0);
    let raster = panel.render([Some(Region::Launch(slot)), None], [Some(dot), None]);
    let img = image::RgbaImage::from_raw(PANEL_W as u32, PANEL_H as u32, raster.px)
        .ok_or("raster size mismatch")?;
    img.save(path).map_err(|e| format!("save panel PNG: {e}"))?;
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--dump-panel") {
        let path = args.get(i + 1).map(|s| s.as_str()).unwrap_or("/tmp/vrdoctor-panel.png");
        match dump_panel(path) {
            Ok(()) => println!("panel written to {path}"),
            Err(e) => { eprintln!("vrdoctor-vr: {e}"); std::process::exit(1); }
        }
        return;
    }
    let opts = Opts {
        probe: args.iter().any(|a| a == "--probe"),
        frames: args.iter().position(|a| a == "--frames")
            .and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()),
    };
    if let Err(e) = run(&opts) {
        eprintln!("vrdoctor-vr: {e}");
        std::process::exit(1);
    }
}
