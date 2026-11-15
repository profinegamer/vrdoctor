# VRDoctor 🩺

> **🤖 Completely vibe coded.** Every line of VRDoctor — the Rust code, this README, the tests, the packaging — was written by an AI assistant (Muse) from plain-language requests. The human behind it directed the features, asked the questions, and shipped the result without writing the code by hand. Bugs are still bugs and reports are welcome — just know who (what) you're reporting them to.

A Rust launcher — **GUI (dark mode by default) + full CLI** — that lets you play **PC VR games on a Meta Quest 3 from Arch or Fedora Linux**. Games run on the PC; the Quest 3 is the display, tracking, and controllers.

It handles **Steam games and custom games that aren't on any store** — any local executable, script, or Windows `.exe`.

> Renamed from `quest3-launcher` (v0.1). Old configs in `~/.config/quest3-launcher/` are still read as a fallback.

## The honest part (read this first)

Meta's own Link / Air Link is **Windows-only**. There is no Meta Linux driver to build on, and no launcher can change that.

What *does* work on Linux, and what VRDoctor automates, is:

| Backend | What it is | Status for Quest 3 on Linux |
|---|---|---|
| **WiVRn** (default, recommended) | Linux-native streamer built on Monado/OpenXR, with xrizer/OpenComposite so OpenVR games work too — **no SteamVR required** | ✅ Quest 3 supported. Native packages: AUR `wivrn-dashboard` on Arch, `wivrn` in Fedora repos, or Flatpak `io.github.wivrn.wivrn` |
| **ALVR** (fallback) | SteamVR streamer, PC server + Quest client app | ✅ Quest 3 supported, widely used. Note: recent community reports say its Linux build can break with very new SteamVR versions — if ALVR misbehaves, use WiVRn |
| **Steam Link** | Valve's Quest app + SteamVR | ⚠️ Works for some Linux users (esp. AMD), not officially promised by Valve for Linux |

"Fully work" depends on the game, not just the launcher: most VR titles work through WiVRn + Proton, but some games fail because of anti-cheat, a game-specific Proton bug, or missing OpenXR features. Controllers, hand tracking, audio and mic are provided by WiVRn/ALVR — VRDoctor starts them correctly and verifies the connection, it does not re-implement encoding/tracking.

## No SteamVR required

**VRDoctor never needs SteamVR.** WiVRn is a complete runtime on its own, so a broken (or absent) SteamVR doesn't matter:

- VR games launch straight into WiVRn; nothing in the default flow installs, starts, or checks SteamVR (Doctor lists it only as "NOT required").
- One gotcha: OpenVR games find their runtime through `~/.config/openvr/openvrpaths.vrpath`, and a SteamVR install registers *itself* there — so a broken SteamVR can hijack games that should go to WiVRn. Doctor detects this, and one command repairs it:

```bash
vrdoctor fix-openvr          # route OpenVR games to xrizer/WiVRn (backup kept)
vrdoctor fix-openvr --undo   # restore the original file
```

- The only backends that still need SteamVR are the optional ALVR and Steam Link fallbacks — they're labelled "(needs SteamVR)" in the GUI.

## Steam games: VR or 2D, auto-detected

VRDoctor classifies every Steam game as **VR**, **VR+2D** (hybrid), or **2D**:

1. A built-in table of known titles (Alyx, Beat Saber, VRChat, …),
2. VR runtime files (OpenVR/OpenXR plugins) inside the game's install folder,
3. An online check against the Steam Store API ("VR Only" / "VR Supported" categories), cached in `~/.config/vrdoctor/steam_vr_cache.conf`.

```bash
vrdoctor list-games            # grouped: VR / VR+2D / 2D
vrdoctor list-games --online   # check unknowns against Steam
vrdoctor classify --game 450390
```

Launching is automatic: `vrdoctor launch` sends flat games to the big screen (2D mode) and VR games to the headset. In the GUI, every Steam game shows a `[VR]` / `[VR+2D]` / `[2D]` badge.

## AppImage + installer (easiest way)

```bash
chmod +x VRDoctor-x86_64.AppImage
./VRDoctor-x86_64.AppImage
```

- **First run opens the installer**, which asks **Arch or Fedora?** (your distro is pre-selected if detected), shows that distro's exact plan, and installs accordingly in a terminal so sudo can prompt. Re-open it any time with `vrdoctor installer`, from the GUI's Setup tab, or use the CLI: `vrdoctor install --distro arch|fedora [--apply]`.
- After installing, the AppImage (and `vrdoctor` with no arguments) opens the main GUI.
- The AppImage bundles the X11/Wayland runtime libraries VRDoctor needs; the VR backends (WiVRn etc.) are installed system-wide by the installer, since they need system services and drivers.
- Build it yourself with `./build-appimage.sh`.

## VR Home environment + 2D games + the bar

**VR Home** — not just one dark space: **7 environment themes**, each a procedurally rendered 360° panorama — `dawn` (bright, calm, Frame-like; the default), `meadow` (green field), `ocean` (coast), `sunset` (warm orange), `night` (deep blue with stars — dark, but not plain black), `studio` (clean white), and `cozy` (warm wood tones). Default brightness is **110%** — a touch brighter, not much:

```bash
vrdoctor home                      # dawn theme, generate + install
vrdoctor home --theme sunset       # pick a different environment
vrdoctor home --brightness 1.15    # re-render brighter/dimmer
```

Panoramas land in `~/.config/vrdoctor/home/vrdoctor-home-<theme>.png` — use one as your desktop/WayVR backdrop on the WiVRn route (no SteamVR involved). If a SteamVR install happens to exist, the panorama is *also* set as its background, as an optional extra. Honest note: this is an environment *backdrop*, not a full interactive 3D home world — Linux's OpenXR stack (WiVRn/Monado) doesn't offer a SteamVR-Home-style world API to hook into.

**2D games** — like the Steam Frame, flat games play on a big floating screen in VR, via the desktop-in-VR overlay (WayVR or wlx-overlay-s):

```bash
vrdoctor play2d --game "Some Game"     # custom or Steam, by name/AppID/path
vrdoctor add-game --name "Flat Game" --path /games/flat/run.sh --mode 2d
```

With a 2D game running, the game fills the big screen; **with no 2D game, the same big panel shows your whole PC desktop**. Steam games also get a **▶ 2D** button in the GUI. `vrdoctor doctor` checks whether an overlay is installed.

**The bar** — a Quest-style bar under the screen: a slim, rounded, always-dark dock (clock, WiVRn status, Quest battery over adb, Games/Doctor/Home shortcuts) that lives at the bottom of your desktop, so inside the VR desktop panel it sits right under the big screen, like Quest's universal bar:

```bash
vrdoctor bar          # also started automatically by 2D mode
```

## GUI

```bash
vrdoctor          # bare launch opens the GUI when a display is available
vrdoctor gui
```

- **Dark mode by default.** The 🌙 toggle in the header switches to light and remembers your choice (`theme=` in `~/.config/vrdoctor/config.conf`).
- **Library tab** — a Steam-style library: a cover-art grid (artwork is fetched from Steam and cached in `~/.cache/vrdoctor/art`; games without art get a coloured initial tile), click a game for its detail bar with a big ▶ Launch button, coloured VR / VR+2D / 2D badges on every game, and a Grid/List view toggle. Search, launch (VR or ▶ 2D), and custom games all live here too.
- **Doctor tab** — the full readiness checklist with pass/fail for each item.
- **Add custom game tab** — add non-store games, with a VR / 2D mode selector (see below).
- **VR Shell tab** — install VR Home (with brightness slider) and start 2D mode / the bar.
- **Setup tab** — the Arch/Fedora install steps, Quest-side steps, and a button to re-open the installer.

## In-headset VR UI — VRDoctor VR 🥽

New in v0.8.0: VRDoctor has a real VR front-end, styled after the Steam Frame's floating-panel library. Connect your headset in WiVRn, open **VRDoctor VR** from WiVRn's own app launcher (it appears there automatically after install), and you're standing in your VR Home in front of a big panel with your whole library — cover art, VR / VR+2D / 2D badges and all. Point a controller at a game and pull the trigger: VRDoctor launches it and steps aside.

```bash
vrdoctor vr                  # register with WiVRn and open the VR UI now
vrdoctor vr --register-only  # just (re)write the WiVRn launcher entry
```

Under the hood it's a native OpenXR app (`vrdoctor-vr`, OpenGL via EGL) that reads the same library as the desktop app — Steam games are sorted VR-first, custom games included. WiVRn finds it through a desktop entry with the `X-WiVRn-VR` category in `~/.local/share/applications` (written by the installer, by `vrdoctor vr`, or shipped by the packages). Honest status: confirmed working on a real Quest 3 over WiVRn (first real-hardware run found the panel rendering upside down — fixed in v0.9.0 with a default-on texture flip, toggleable in Settings).

## Updating

VRDoctor updates itself. The app checks GitHub on startup; when a newer
release exists, a banner offers **Download & install**, then **Restart
now**. You can also check manually in **Settings → Updates**, or from
the terminal:

```bash
vrdoctor update          # check + download + replace the AppImage in place
vrdoctor update --check  # only tell me if there's a new version
```

The update downloads the new AppImage, sanity-checks it (ELF, sane
size) and atomically replaces the old file — your settings, library
and custom games are untouched. (Self-update works for the AppImage
build; package installs update through their package manager.)

## Settings (PC tab + in-headset page)

Both apps share one settings file (`~/.config/vrdoctor/config.conf`) and
one Settings UI — the desktop app's **🎛 Settings** tab and, inside the
headset, **VRDoctor VR → ⚙ Settings**:

- **Environment picker** — switch the VR Home world (dawn, meadow,
  ocean, sunset, night, studio, cozy) + brightness. On the PC it applies
  when you click *Apply environment*; in the headset the new world is
  built and swapped in live.
- **Refresh rate (Hz)** — 72 / 80 / 90 / 120. VRDoctor VR requests the
  rate from the runtime at startup (XR_FB_display_refresh_rate) when
  the runtime supports it. For games, the streaming rate is picked in
  the WiVRn app on the Quest itself — no PC app can set that for them.
- **Resolution scale** — 50–200%, how sharp VRDoctor VR itself renders.
  (Game streaming resolution likewise lives in the WiVRn app on the
  Quest.)
- **Display flips** — if the panel or the environment ever looks upside
  down in the headset, flip either one, live, from either app. The
  panel flip is on by default: the compositor samples the panel texture
  upside down otherwise (confirmed on a real Quest 3).
- **Panel size + Recenter** (in-headset) — make the floating panel
  bigger or smaller, and re-place it in front of you, from the header's
  **◎ Recenter** button.
- Plus the basics: dark mode, accent colour (teal/red), default
  backend.

## Flatpak Steam

If your Steam is the Flatpak (`com.valvesoftware.Steam`), VRDoctor uses it as-is: it detects which Steam actually holds your games, launches through `flatpak run com.valvesoftware.Steam`, and **never installs a duplicate native Steam** (the install plan skips the Steam package when any Steam is already present). Because Flatpak Steam is sandboxed, it also can't see WiVRn by default — `vrdoctor install` applies the documented visibility fix automatically for Flatpak users, and you can re-apply it any time with:

```bash
vrdoctor fix-steam
```

That grants Steam read access to `xdg-run/wivrn`, `xdg-config/openxr` and `xdg-config/openvr` and links your OpenXR config into the sandbox.

## Uninstalling

In the app: **Setup tab → Uninstall** (two clicks, with a warning). From a terminal:

```bash
vrdoctor uninstall          # lists what would be removed, changes nothing
vrdoctor uninstall --yes    # actually removes it
```

Both remove VRDoctor's own files — settings, custom games, VR Home, caches, and its menu entries (including the WiVRn one) — and then you just delete the AppImage file itself. The system packages it installed (WiVRn and friends) are shared with other apps and are left in place; the uninstall output tells you how to remove those too if you want them gone.

Building the GUI needs X11/Wayland headers on some systems — Fedora: `sudo dnf install libXcursor-devel libXi-devel libXrandr-devel libxkbcommon-devel wayland-devel`. Arch usually has what's needed via base-devel and desktop deps.

## Custom (non-store) games — almost any file type

Add **any** game, no store required. Point VRDoctor at the game file, an archive, or a folder — it works out the rest:

| File | How it runs |
|---|---|
| Native executable (ELF, shebang, or +x — any extension or none) | Directly |
| `.AppImage` | Directly (chmod +x applied if needed) |
| `.sh` / `.bash` | bash |
| `.py`, `.pl`, `.rb`, `.js` | python3 / perl / ruby / node (must be installed) |
| `.jar` | `java -jar` (Java runtime must be installed) |
| `.love` | LÖVE runtime |
| `.exe` / `.com`, `.bat` / `.cmd` | **Proton** (auto-detected from Steam, per-game compat data under `~/.config/vrdoctor/compatdata/`) or **Wine** fallback; specific Proton can be set per game. When **umu-launcher** is installed, games run through it — Proton inside the same runtime container Steam uses (a bare `proton run` skips that container, and some games, e.g. SUPERHOT VR, then fail with "Failed to initialize graphics") |
| `.msi` | Installer via Proton/Wine `msiexec` — install, then add the installed game |
| `.desktop` | Runs its `Exec=` line |
| `.apk` | **Sideloaded onto the Quest 3** via adb (it's a native Quest game, not streamed) |
| `.flatpak` / `.flatpakref` | Installed with `flatpak install --user` |
| Archives: `.zip`, `.tar`, `.tar.gz`/`.tgz`, `.tar.xz`, `.tar.bz2`, `.tar.zst`, `.7z`, `.rar` | Extracted to `~/.local/share/vrdoctor/library/<game>/`, and the best executable inside is registered (native builds preferred over `.exe`). `.7z`/`.rar` need `p7zip` or `bsdtar` installed |
| A **folder** containing a game | Searched up to 3 levels deep for the best executable, same preference order |

Not supported, deliberately: game ROMs/disc images (they need an emulator configured per system — add the emulator as the game and the ROM as an argument instead, e.g. `--path /usr/bin/<emulator> --args "/games/rom.iso"`), and random documents/media — VRDoctor will tell you plainly if a file isn't runnable instead of guessing.

In the GUI: **Add custom game** tab — name, executable path, arguments, working directory, optional backend/Proton overrides.

From the CLI:

```bash
vrdoctor add-game --name "My Indie Game" --path /games/indie/run.sh --args "--fullscreen"
vrdoctor add-game --name "Old Windows Game" --path /games/old/game.exe
vrdoctor add-game --name "Itch Game" --path ~/Downloads/itch-game.zip   # archive: extracted, executable found
vrdoctor add-game --name "Folder Game" --path ~/Games/some-folder       # folder: executable found inside
vrdoctor add-game --name "Quest Native" --path ~/Downloads/game.apk     # apk: sideloaded to the Quest on launch
vrdoctor list-games        # Steam games + [custom] games together
vrdoctor launch --game "My Indie Game"
vrdoctor remove-game --name "My Indie Game"
```

Custom games live in `~/.config/vrdoctor/custom_games.conf` — plain text, one block per game, safe to edit by hand:

```
name=My Indie Game
path=/games/indie/run.sh
args=--fullscreen
```

`launch` resolves in this order: custom game name → Steam AppID → filesystem path → Steam game name.

## Install

```bash
./install.sh          # builds with Cargo, installs vrdoctor to ~/.local/bin + desktop entry
vrdoctor setup        # shows the exact Arch/Fedora steps
vrdoctor setup --apply  # …or runs them for you (sudo prompts)
vrdoctor doctor       # verify everything
```

Packaging included: `PKGBUILD` (Arch) and `vrdoctor.spec` (Fedora RPM).

## One-time Quest 3 setup

1. Install the **WiVRn client** on the headset (Meta Store / SideQuest). Its version **must match** the PC WiVRn server.
   - Using ALVR instead? Install the ALVR client app on the headset.
2. Same network: PC wired to the router, Quest on **5 GHz / 6 GHz Wi-Fi** on the same network. Or use USB.
3. Allow the firewall ports: **UDP 5353** (mDNS/Avahi discovery) and **TCP/UDP 9757** (WiVRn). ALVR uses **TCP/UDP 9943–9944**.
4. Make sure `avahi-daemon` is running (`vrdoctor doctor` checks this).

## CLI reference

```bash
vrdoctor gui
vrdoctor doctor
vrdoctor devices                  # Quest 3 over USB (adb) — optional for wireless
vrdoctor list-games
vrdoctor launch                   # start backend only
vrdoctor launch --game 546560     # Half-Life: Alyx by AppID
vrdoctor launch --game "alyx"     # …by Steam or custom name (substring match)
vrdoctor launch --backend alvr --game 546560
vrdoctor launch --game /path/to/game-binary
vrdoctor config                   # ~/.config/vrdoctor/config.conf (backend, refresh, theme, proton)
```

Quest 3 refresh rates 72 / 90 / 120 Hz are set in the WiVRn/ALVR dashboard; VRDoctor's config defaults to `backend=wivrn`, `default_refresh=90`, `theme=dark`.

## Troubleshooting quick hits

- **Headset not found (wireless):** avahi-daemon running? Same subnet? UDP 5353 open? Open the WiVRn app *on the Quest first*, then launch.
- **Black screen in an OpenVR game:** usually the OpenVR→OpenXR layer — make sure xrizer/OpenComposite is present (the WiVRn Flatpak bundles both). If SteamVR is installed, also run `vrdoctor fix-openvr` so it can't hijack the game.
- **A game opens in SteamVR instead of WiVRn:** that's the openvrpaths hijack — `vrdoctor fix-openvr`.
- **Custom .exe says no Proton/Wine:** install the game once via Steam → *Add a Non-Steam Game* so a Proton build exists, set `--proton /path/to/proton`, or install Wine.
- **SteamVR game + ALVR crash on load:** try WiVRn, or SteamVR's "previous" beta branch.
- **Stutter:** wired PC→router, 5/6 GHz Wi-Fi, lower refresh to 72/80 Hz or bitrate in the WiVRn dashboard. AMD GPUs are generally the smoothest on Linux VR; NVIDIA works but keep drivers current.

## Layout

```
src/main.rs   CLI, distro/Steam/headset detection, doctor, setup, launch, custom games
src/gui.rs    egui GUI (dark by default)
install.sh    build + install
PKGBUILD      Arch package
vrdoctor.spec Fedora RPM spec
vrdoctor.desktop
```

## Tests

```bash
cargo test   # 21 tests: distro detection, per-distro install plans, Steam manifests,
             # adb, Quest 3 ID, custom games + file kinds, .desktop parsing,
             # vrsettings background editing, panorama rendering (all 7 themes),
             # dark default, openvrpaths parsing, Steam Store VR-category parsing,
             # known-title classification…

scripts/test-distro.sh arch 20     # full CLI battery ×20 against an Arch profile
scripts/test-distro.sh fedora 20   # full CLI battery ×20 against a Fedora profile
scripts/test-free-games.sh arch    # VR/2D detection vs 20 real free Steam games
scripts/test-free-games.sh fedora  # (The Lab, VRChat, Dota 2, …) — 20/20 on both
```

The distro harness points the binary at fixture os-release files (`tests/os-release.arch` / `.fedora`, via `VRDOCTOR_OS_RELEASE`), so the Arch and Fedora code paths — installer plan, doctor, setup — run exactly as on those distros; GPU/headset checks still reflect the real test machine. Both profiles pass 20/20 (debug and release binaries).
