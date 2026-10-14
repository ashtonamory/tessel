#!/usr/bin/env bash
# Remove what scripts/install.sh installed. Usage: scripts/uninstall.sh [--dry-run]
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

PREFIX="${PREFIX:-$HOME/.local}"
DESTDIR="${DESTDIR:-}"
root="$DESTDIR$PREFIX"

run() {
  if ((dry)); then
    printf '+'
    printf ' %q' "$@"
    printf '\n'
  else
    "$@"
  fi
}

if command -v cargo >/dev/null && [[ -f "$root/.crates.toml" ]]; then
  run cargo uninstall --root "$root" app || true
fi
run rm -f \
  "$root/bin/tessel" \
  "$root/share/applications/tessel.desktop" \
  "$root/share/icons/hicolor/scalable/apps/tessel.svg" \
  "$root/share/mime/packages/tessel-mime.xml"

if [[ -z "$DESTDIR" ]]; then
  if command -v update-mime-database >/dev/null && [[ -d "$PREFIX/share/mime" ]]; then
    run update-mime-database "$PREFIX/share/mime"
  fi
  if command -v update-desktop-database >/dev/null && [[ -d "$PREFIX/share/applications" ]]; then
    run update-desktop-database "$PREFIX/share/applications"
  fi
fi
echo "Done. Per-user recovery files in ~/.local/state/tessel are left alone."
