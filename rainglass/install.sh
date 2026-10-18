#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# rainglass: build it and install the lock screen (rainlock). Idempotent: run
# it again after pulling or editing to deploy a new build.
#
#     bash ~/git/New-Niri-minimal-dots/rainglass/install.sh
#
# Asks for sudo once, for /etc/pam.d/rainlock.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
  echo "!! cargo not found: sudo pacman -S rust, then run this again." >&2
  exit 1
fi

echo "==> Building rainlock (release)"
cargo build --release --manifest-path "$HERE/Cargo.toml" -p rainlock
mkdir -p "$HOME/.local/bin"
ln -sfn "$HERE/target/release/rainlock" "$HOME/.local/bin/rainlock"
echo "    linked ~/.local/bin/rainlock"

# rainlock refuses to start without its PAM service (PAM would fall through to
# /etc/pam.d/other, pam_deny on Arch, and the session could never unlock), so
# niri-lock/lock.sh keeps using gtklock until this file is in place.
echo "==> PAM service /etc/pam.d/rainlock"
if cmp -s "$HERE/rainlock/pam/rainlock" /etc/pam.d/rainlock; then
  echo "    already installed"
else
  sudo install -Dm644 "$HERE/rainlock/pam/rainlock" /etc/pam.d/rainlock
  echo "    installed"
fi

# The lock shows the session's wallpaper. autostart.sh and set-wallpaper record
# it from now on; seed it from the running swaybg so it works before the next
# login too.
state="${XDG_STATE_HOME:-$HOME/.local/state}"
if [[ ! -s $state/niri-wallpaper ]]; then
  current=$(pgrep -a swaybg | sed -n 's/.* -i \(.*\) -m .*/\1/p' | head -n1)
  if [[ -n $current && -f $current ]]; then
    mkdir -p "$state"
    printf '%s\n' "$current" > "$state/niri-wallpaper"
    echo "==> Lock screen picture: $current"
  fi
fi

cat <<'EOF'

==> Done.
    - Lock: Mod+Alt+Escape (and after 10 minutes idle, and before sleep).
    - If the lock ever wedges: Mod+Shift+Alt+Escape hands it to gtklock.
    - Settings: ~/.config/rainlock/config (rainlock --help lists the keys),
      e.g.  density=0.5  or  image=~/Pictures/wallpapers/rogue.jpg
    - Try the look without locking:
        cargo run --release --manifest-path rainglass/Cargo.toml --example preview -- \
          --image ~/Pictures/wallpapers/rogue.jpg --clock --png /tmp/rain.png
EOF
