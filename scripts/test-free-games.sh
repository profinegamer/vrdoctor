#!/usr/bin/env bash
# Test VRDoctor's Steam VR/2D auto-detection against 20 real free games,
# under a distro profile:  scripts/test-free-games.sh <arch|fedora> [binary]
# Each game is classified with `vrdoctor classify` in an isolated HOME.
# VR titles resolve via the built-in known-titles table; flat titles fall
# through to the live Steam Store API check.
set -u
DISTRO="${1:?usage: test-free-games.sh <arch|fedora> [binary]}"
BIN="${2:-./target/release/vrdoctor}"
FIXTURE="tests/os-release.${DISTRO}"
[ -f "$FIXTURE" ] || { echo "no fixture: $FIXTURE"; exit 2; }
[ -x "$BIN" ] || { echo "binary not executable: $BIN"; exit 2; }
export VRDOCTOR_OS_RELEASE="$(pwd)/$FIXTURE"
pass=0; fail=0
while IFS=$'\t' read -r appid name expected; do
  case "$appid" in ''|\#*) continue;; esac
  TMPHOME="$(mktemp -d)"
  out="$(HOME="$TMPHOME" "$BIN" classify --game "$appid" 2>&1)"
  rm -rf "$TMPHOME"
  got="$(printf '%s' "$out" | sed -n 's/^[0-9]*: \([^(]*\)(.*/\1/p' | sed 's/ *$//')"
  if [ "$got" = "$expected" ]; then
    pass=$((pass+1)); printf 'PASS  %-10s %-22s -> %s\n' "$appid" "$name" "$got"
  else
    fail=$((fail+1)); printf 'FAIL  %-10s %-22s expected %s, got: %s\n' "$appid" "$name" "$expected" "$out"
  fi
done < scripts/free-games.tsv
echo "free-games [$DISTRO]: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
