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

echo "Building and installing to $root/bin"
if ! run cargo install --offline --locked --path crates/app --root "$root" --force; then
  echo "Offline install failed; retrying with network access." >&2
  run cargo install --locked --path crates/app --root "$root" --force
fi

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
