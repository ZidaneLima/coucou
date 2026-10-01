#!/usr/bin/env bash
# Builds Coucou from this checkout and installs it into your home directory.
#
# No root, no package manager, nothing outside ~/.local — which is what an
# Omarchy or plain Arch user usually wants, and what the AUR package does for
# the system-wide case.
#
#   ./packaging/arch/install.sh              # build, install, offer autostart
#   ./packaging/arch/install.sh --uninstall  # remove everything it installed
#
# Requirements (Arch names):
#   sudo pacman -S --needed base-devel cargo nodejs npm pkgconf \
#     webkit2gtk-4.1 gtk3 gtk-layer-shell libappindicator-gtk3 librsvg \
#     openssl dbus patchelf
#   # for Mochi's 28 sounds:
#   sudo pacman -S --needed gst-plugins-good pipewire

set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
LIBDIR="$PREFIX/lib/coucou"
BINDIR="$PREFIX/bin"
APPDIR="$PREFIX/share/applications"
AUTOSTART="$HOME/.config/autostart"
ICONDIR="$PREFIX/share/icons/hicolor"

# Resolves the repository root from this script's own location, so the script
# works from anywhere and from a symlink into ~/bin.
SCRIPT="${BASH_SOURCE[0]}"
while [ -L "$SCRIPT" ]; do
  TARGET="$(readlink "$SCRIPT")"
  case "$TARGET" in
    /*) SCRIPT="$TARGET" ;;
    *) SCRIPT="$(dirname "$SCRIPT")/$TARGET" ;;
  esac
done
ROOT="$(cd "$(dirname "$SCRIPT")/../.." && pwd)"

say() { printf '  %s\n' "$*"; }
die() { printf '\nerror: %s\n\n' "$*" >&2; exit 1; }

installed_files() {
  cat <<EOF
$BINDIR/coucou
$LIBDIR/coucou
$LIBDIR/coucou-hook
$APPDIR/coucou.desktop
$ICONDIR/256x256/apps/coucou.png
$ICONDIR/32x32/apps/coucou.png
$AUTOSTART/coucou-autostart.desktop
EOF
}

uninstall() {
  local removed=0
  while read -r file; do
    if [ -e "$file" ] || [ -L "$file" ]; then
      rm -f "$file" && say "removed $file" && removed=1
    fi
  done <<< "$(installed_files)"
  rmdir "$ICONDIR/256x256/apps" "$ICONDIR/32x32/apps" 2>/dev/null || true

  # ~/.claude/settings.json is Coucou's to edit and to undo, never to delete:
  # the settings window's "remove hooks" is the only thing that touches it.
  say ""
  say "Your settings are untouched:"
  say "  config    $PREFIX/share/coucou/settings.json  (or ~/.config/coucou/settings.json)"
  say "  relay     ~/.local/share/coucou/bin/coucou-hook"
  say "  Claude    ~/.claude/settings.json"
  say ""
  say "Use Coucou's Settings window to remove the hooks and the opencode plugin."
  [ "$removed" = 1 ] || say "Nothing was installed."
}

[ "${1-}" = "--uninstall" ] && { uninstall; exit 0; }

# ── What is missing ────────────────────────────────────────────────────────────

missing=()
command -v cargo >/dev/null 2>&1 || missing+=("cargo")
command -v node  >/dev/null 2>&1 || missing+=("nodejs")
command -v npm   >/dev/null 2>&1 || missing+=("npm")
command -v pkg-config >/dev/null 2>&1 || missing+=("pkgconf")

if [ "${#missing[@]}" -gt 0 ]; then
  die "missing build tools: ${missing[*]}
  On Arch:  sudo pacman -S --needed base-devel cargo nodejs npm pkgconf"
fi

if ! pkg-config --exists webkit2gtk-4.1 gtk+-3.0 gtk-layer-shell 2>/dev/null; then
  die "missing the development headers. On Arch:
  sudo pacman -S --needed webkit2gtk-4.1 gtk3 gtk-layer-shell libappindicator-gtk3 librsvg openssl dbus patchelf"
fi

# ── Build ──────────────────────────────────────────────────────────────────────

printf '\n  Coucou — building from %s\n\n' "$ROOT"

cd "$ROOT/windows"

say "building the relay (coucou-hook)"
cargo build --release --manifest-path hook/Cargo.toml

say "installing the front-end dependencies"
npm ci

say "building the front-end"
npm run build

say "building the app (this is the slow part)"
npm run tauri -- build --no-bundle

[ -x target/release/coucou ] || die "the build produced no target/release/coucou"
[ -x target/release/coucou-hook ] || die "the build produced no target/release/coucou-hook"

# ── Install ────────────────────────────────────────────────────────────────────

say ""
say "installing into $PREFIX"
mkdir -p "$LIBDIR" "$BINDIR" "$APPDIR" "$ICONDIR/256x256/apps" "$ICONDIR/32x32/apps"

install -m755 target/release/coucou "$LIBDIR/coucou"
# The relay goes next to the binary: that is the first place Coucou looks when it
# copies its own relay into ~/.local/share/coucou/bin on first launch.
install -m755 target/release/coucou-hook "$LIBDIR/coucou-hook"
ln -sf "$LIBDIR/coucou" "$BINDIR/coucou"

install -m644 "$ROOT/packaging/arch/coucou.desktop" "$APPDIR/coucou.desktop"
install -m644 "$ROOT/windows/src-tauri/icons/icon.png" "$ICONDIR/256x256/apps/coucou.png"
install -m644 "$ROOT/windows/src-tauri/icons/32x32.png" "$ICONDIR/32x32/apps/coucou.png"

update-desktop-database "$APPDIR" 2>/dev/null || true
say "installed $BINDIR/coucou"

# ── Autostart ──────────────────────────────────────────────────────────────────

if [ -e "$AUTOSTART/coucou-autostart.desktop" ]; then
  say "autostart already on"
elif [ -t 0 ] && printf '  start Coucou at login? [y/N] ' | grep -qi '^y'; then
  mkdir -p "$AUTOSTART"
  install -m644 "$ROOT/packaging/arch/coucou-autostart.desktop" \
    "$AUTOSTART/coucou-autostart.desktop"
  say "autostart on — remove $AUTOSTART/coucou-autostart.desktop to turn it off"
else
  say "autostart off — to turn it on:"
  say "  mkdir -p $AUTOSTART && cp $ROOT/packaging/arch/coucou-autostart.desktop $AUTOSTART/"
fi

cat <<EOF

  Coucou is installed.

    run it          coucou
    remove it       ./packaging/arch/install.sh --uninstall
    the log         ~/.local/share/coucou/coucou.log

  First launch: open Coucou's Settings window to install the Claude Code hooks
  and the opencode plugin. It shows the diff and waits for your click before it
  writes anything.

  On Hyprland the island sits on the top layer, next to your bar. If the bar is
  drawn over Mochi's head, give it room with COUCOU_TOP_MARGIN — the number is
  your bar's height in logical pixels:

    Hyprland        exec-once = env COUCOU_TOP_MARGIN=40 coucou
                    (in ~/.config/hypr/user.conf)
    autostart       edit Exec=coucou in
                    ~/.config/autostart/coucou-autostart.desktop
    anything else   COUCOU_TOP_MARGIN=40 coucou
EOF
