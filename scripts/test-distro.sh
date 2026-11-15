#!/usr/bin/env bash
# VRDoctor distro test harness.
#
#   scripts/test-distro.sh arch|fedora [runs] [binary]
#
# Runs the full CLI battery <runs> times (default 20) against a simulated
# Arch or Fedora system. The simulation is honest about what it is: the
# binary reads its os-release from VRDOCTOR_OS_RELEASE (fixtures in
# tests/), so every distro-specific code path — installer plan, doctor,
# setup — runs exactly as it would on that distro. Hardware checks
# (GPU/headset) still reflect the real test machine.
set -u
DISTRO="${1:?usage: test-distro.sh arch|fedora [runs] [binary]}"
RUNS="${2:-20}"
cd "$(dirname "$0")/.."
BIN="${3:-${BIN:-target/debug/vrdoctor}}"
[ -x "$BIN" ] || { echo "binary not found/executable: $BIN (build first: cargo build)"; exit 2; }
BIN="$(readlink -f "$BIN")"
FIXTURE="tests/os-release.$DISTRO"
[ -f "$FIXTURE" ] || { echo "no fixture: $FIXTURE"; exit 2; }

WORK="$(mktemp -d)"
export HOME="$WORK/home" XDG_CONFIG_HOME="$WORK/home/.config" XDG_DATA_HOME="$WORK/home/.local/share"
export VRDOCTOR_OS_RELEASE="$(readlink -f "$FIXTURE")"
mkdir -p "$HOME/games"
printf '#!/bin/sh\necho ran >> "%s/games/marker.txt"\n' "$HOME" > "$HOME/games/loop.sh"
chmod +x "$HOME/games/loop.sh"

pass=0; fail=0
for i in $(seq 1 "$RUNS"); do
  ok=1
  # 1. Installer plan matches the distro.
  plan="$("$BIN" install --distro "$DISTRO" 2>&1)"
  case "$DISTRO" in
    arch)   echo "$plan" | grep -q "pacman" && echo "$plan" | grep -q "wivrn-dashboard" || ok=0 ;;
    fedora) echo "$plan" | grep -q "dnf install" && echo "$plan" | grep -q "wivrn" || ok=0 ;;
  esac
  # 2. Doctor recognises the distro as supported.
  "$BIN" doctor 2>&1 | grep -q "Distro" || ok=0
  # 3. VR Home panoramas render (default dawn + a second theme).
  "$BIN" home --brightness 1.10 >/dev/null 2>&1 || ok=0
  [ -s "$XDG_CONFIG_HOME/vrdoctor/home/vrdoctor-home-dawn.png" ] || ok=0
  "$BIN" home --theme sunset >/dev/null 2>&1 || ok=0
  [ -s "$XDG_CONFIG_HOME/vrdoctor/home/vrdoctor-home-sunset.png" ] || ok=0
  # 3b. Steam VR/2D auto-detection (offline known titles).
  "$BIN" classify --game 450390 --offline 2>&1 | grep -q "VR (known" || ok=0
  "$BIN" classify --game 438100 --offline 2>&1 | grep -q "VR+2D (known" || ok=0
  # 3c. fix-openvr removes a SteamVR-only openvrpaths hijack (backup kept).
  mkdir -p "$HOME/.config/openvr"
  printf '{ "runtime" : [ "/fake/Steam/steamapps/common/SteamVR" ] }' > "$HOME/.config/openvr/openvrpaths.vrpath"
  "$BIN" doctor 2>&1 | grep -q "OpenVR routing" || ok=0
  "$BIN" fix-openvr >/dev/null 2>&1 || ok=0
  [ ! -f "$HOME/.config/openvr/openvrpaths.vrpath" ] || ok=0
  [ -f "$HOME/.config/openvr/openvrpaths.vrpath.vrdoctor-bak" ] || ok=0
  "$BIN" fix-openvr --undo >/dev/null 2>&1 || ok=0
  [ -f "$HOME/.config/openvr/openvrpaths.vrpath" ] || ok=0
  # 4. Custom game add + list + launch actually runs it.
  "$BIN" add-game --name "Loop Game" --path "$HOME/games/loop.sh" >/dev/null 2>&1 || ok=0
  "$BIN" list-games 2>&1 | grep -q "Loop Game" || ok=0
  "$BIN" launch --game "Loop Game" >/dev/null 2>&1 || ok=0
  # 5. 2D mode launch path runs too (bar/overlay degrade gracefully headless).
  "$BIN" play2d --game "Loop Game" >/dev/null 2>&1 || ok=0
  sleep 0.3
  lines=$(wc -l < "$HOME/games/marker.txt" 2>/dev/null || echo 0)
  [ "$lines" -ge $((i * 2)) ] || ok=0
  if [ "$ok" = 1 ]; then pass=$((pass+1)); else fail=$((fail+1)); echo "  run $i: FAIL"; fi
done
echo "$DISTRO: $pass/$RUNS iterations passed, $fail failed (workdir: $WORK)"
[ "$fail" = 0 ]
