#!/usr/bin/env bash
# Install tessel as a desktop app: binary, .desktop entry, icon and
# MIME type. Usage: scripts/install.sh [--dry-run]
# Env: PREFIX (default ~/.local), DESTDIR (staging root, skips cache updates).
set -euo pipefail

dry=0
for arg in "$@"; do
  case "$arg" in
    --dry-run | -n) dry=1 ;;
    -h | --help)
      echo "usage: $0 [--dry-run]   (env: PREFIX=~/.local, DESTDIR=)"
      exit 0
      ;;
    *)
      echo "unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
DESTDIR="${DESTDIR:-}"
root="$DESTDIR$PREFIX"
bin="$PREFIX/bin/tessel"

run() {
  if ((dry)); then
    printf '+'
    printf ' %q' "$@"
    printf '\n'
  else
    "$@"
  fi
}

cd "$repo"

# Oldest rustc the locked dependencies build with (eframe/egui 0.36).
msrv="1.95"

die() {
  printf 'error: %s\n' "$@" >&2
  exit 1
}

# Check the build tools before a long build, and say how to get each one.
preflight() {
  if ! command -v cargo >/dev/null; then
    if command -v omarchy-install-dev-env >/dev/null; then
      hint="omarchy-install-dev-env rust   (then open a new terminal)"
    elif command -v pacman >/dev/null; then
      hint="sudo pacman -S --needed rustup && rustup default stable"
    else
      hint="see https://rustup.rs"
    fi
    die "cargo (the Rust toolchain) is not installed." "Install it with: $hint"
  fi
  if ! cargo --version >/dev/null 2>&1; then
    # rustup is installed but has no toolchain yet.
    die "cargo is installed but cannot run:" "  $(cargo --version 2>&1 | head -n 1 | sed "s/^error: //")" \
      "Install a toolchain with: rustup default stable"
  fi
  local have
  have="$(rustc --version | awk '{print $2}')"
  if [[ "$(printf '%s\n%s\n' "$msrv" "${have%%-*}" | sort -V | head -n 1)" != "$msrv" ]]; then
    if command -v rustup >/dev/null; then
      hint="rustup update stable"
    else
      hint="update your rust package"
    fi
    die "rustc $have is too old; tessel needs $msrv or newer." "Update it with: $hint"
  fi
  if ! command -v cc >/dev/null; then
    die "no C compiler (cc) found." "Install it with: sudo pacman -S --needed base-devel   (Debian/Ubuntu: build-essential)"
  fi
  if ! command -v pkg-config >/dev/null || ! pkg-config --exists fontconfig; then
    die "pkg-config or the fontconfig development files are missing." \
      "Install them with: sudo pacman -S --needed pkgconf fontconfig   (Debian/Ubuntu: pkg-config libfontconfig1-dev)"
  fi
}

preflight

echo "Building and installing to $root/bin"
echo "(a first build downloads ~350 MB of crates and takes a few minutes)"
# Build offline only when every locked crate is already downloaded; a fresh
# machine goes straight to the normal build instead of showing an error first.
offline=()
if cargo fetch --locked --offline >/dev/null 2>&1; then
  offline=(--offline)
fi
run cargo install "${offline[@]}" --locked --path crates/app --root "$root" --force

apps="$root/share/applications"
icons="$root/share/icons/hicolor/scalable/apps"
mime="$root/share/mime/packages"
run install -d "$apps" "$icons" "$mime"

# Exec gets the absolute binary path so the launcher need not have it on PATH.
desktop="$apps/tessel.desktop"
if ((dry)); then
  echo "+ write $desktop (Exec=$bin %f)"
else
  sed "s|^Exec=.*|Exec=$bin %f|" packaging/tessel.desktop >"$desktop"
  chmod 644 "$desktop"
fi
run install -m 644 packaging/tessel.svg "$icons/tessel.svg"
run install -m 644 packaging/tessel-mime.xml "$mime/tessel-mime.xml"

if [[ -z "$DESTDIR" ]]; then
  if command -v update-mime-database >/dev/null; then
    run update-mime-database "$PREFIX/share/mime"
  fi
  if command -v update-desktop-database >/dev/null; then
    run update-desktop-database "$PREFIX/share/applications"
  fi
  if command -v gtk-update-icon-cache >/dev/null && [[ -f "$PREFIX/share/icons/hicolor/index.theme" ]]; then
    run gtk-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" || true
  fi
  if command -v xdg-mime >/dev/null; then
    run xdg-mime default tessel.desktop application/x-tessel
  fi
else
  echo "DESTDIR is set: skipping database updates and xdg-mime."
fi

case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *) echo "warning: $PREFIX/bin is not on PATH; add it to run tessel from a shell." >&2 ;;
esac
echo "Done."
