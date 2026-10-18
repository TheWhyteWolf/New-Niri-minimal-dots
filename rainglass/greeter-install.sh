#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Switch the login screen to greetd + raingreet (the rainy-window greeter that
# matches rainlock; tuigreet stays installed as the fallback). Separate from
# install.sh because it replaces the display manager: run it once,
# deliberately. Idempotent: re-run it to deploy a rebuilt raingreet, or with
# --image to change the picture.
#
#     bash ~/git/New-Niri-minimal-dots/rainglass/greeter-install.sh [--image PATH]
#
# The picture defaults to the desktop's current wallpaper. Needs sudo.
# Adapted from LifeBranch's greeter-install.sh.
#
# Rollback (from a TTY, Ctrl+Alt+F3):
#   back to tuigreet: sudo install -Dm644 rainglass/greetd/config-tuigreet.toml /etc/greetd/config.toml
#                     sudo systemctl restart greetd
#   back to the old display manager (this script prints its name; LightDM on
#   the machine this was written on):
#                     sudo systemctl disable greetd && sudo systemctl enable <it> && reboot
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

image=
while (( $# )); do
  case $1 in
    --image) image=${2:?--image needs a path}; shift 2 ;;
    *) echo "!! unknown option '$1' (the only one is --image PATH)" >&2; exit 2 ;;
  esac
done

# id -un, not $USER: $USER is root under sudo, and the PAM check below would
# then test root's locked account and fail a good stack.
me=$(id -un)
if [[ $me == root ]]; then
  echo "!! Run this as your own user, not with sudo — it sudos where it needs to." >&2
  exit 1
fi

ask_yn() {  # question default(y/n)
  local a
  read -rp "    $1 [$([[ $2 == y ]] && echo Y/n || echo y/N)] " a || a=
  a=${a:-$2}
  [[ $a == [Yy]* ]]
}

if [[ -z $image ]]; then
  state="${XDG_STATE_HOME:-$HOME/.local/state}/niri-wallpaper"
  [[ -s $state ]] && image=$(<"$state")
fi
if [[ -z $image || ! -f $image ]]; then
  echo "!! No picture: pass --image PATH (the desktop's wallpaper wasn't recorded yet)." >&2
  exit 1
fi
ext=${image##*.}
ext=${ext,,}
case $ext in
  jpg|jpeg|png|webp) ;;
  *) echo "!! $image: use a jpg, png or webp" >&2; exit 1 ;;
esac

echo "==> Installing greetd + tuigreet (fallback) + cage"
sudo pacman -S --needed greetd greetd-tuigreet cage
# pamtester (AUR) is what proves the PAM stack works before anyone reboots into it.
if ! command -v pamtester >/dev/null 2>&1 && command -v yay >/dev/null 2>&1; then
  yay -S --needed pamtester || echo "    (pamtester didn't install; the PAM stack can't be verified)"
fi

echo "==> Building raingreet (release)"
cargo build --release --manifest-path "$HERE/Cargo.toml" -p raingreet
# /usr/local/bin, not ~/.local/bin: it runs as the greeter user.
sudo install -Dm755 "$HERE/target/release/raingreet" /usr/local/bin/raingreet
sudo install -Dm755 "$HERE/greetd/raingreet-cage" /usr/local/bin/raingreet-cage

# The greeter user can't read anyone's home, so the picture gets its own copy.
echo "==> Picture behind the glass: $image"
sudo install -Dm644 "$image" "/usr/share/raingreet/background.$ext"
conf=/etc/raingreet/config
if [[ ! -f $conf ]]; then
  printf '%s\n' "# raingreet settings (raingreet --help lists the keys)." "image=/usr/share/raingreet/background.$ext" \
    | sudo install -Dm644 /dev/stdin "$conf"
else
  sudo sed -i "s|^image=.*|image=/usr/share/raingreet/background.$ext|" "$conf"
  sudo grep -q '^image=' "$conf" || echo "image=/usr/share/raingreet/background.$ext" | sudo tee -a "$conf" >/dev/null
fi
# Remembers the last user and session between boots.
sudo install -d -o greeter -g greeter -m 700 /var/cache/raingreet
# tuigreet (the fallback) needs one too, for --remember.
sudo install -d -o greeter -g greeter -m 755 /var/cache/tuigreet

echo "==> Installing /etc/greetd/config.toml"
if [[ -f /etc/greetd/config.toml && ! -f /etc/greetd/config.toml.bak ]] \
   && ! cmp -s "$HERE/greetd/config.toml" /etc/greetd/config.toml; then
  sudo cp /etc/greetd/config.toml /etc/greetd/config.toml.bak
  echo "    backed up the previous config to config.toml.bak"
fi
sudo install -Dm644 "$HERE/greetd/config.toml" /etc/greetd/config.toml

# PAM: greetd's own stack plus gnome-keyring auto-unlock, as LightDM's and SDDM's have, so
# logging in still unlocks the login keyring. The live stack is never the
# thing under test: stage it under a scratch name, authenticate against that,
# and only promote it once it answers.
echo "==> Installing /etc/pam.d/greetd (login + gnome-keyring unlock)"
VERIFY_SVC=greetd-verify
trap 'sudo rm -f "/etc/pam.d/$VERIFY_SVC"' EXIT
sudo install -Dm644 "$HERE/greetd/pam/greetd" "/etc/pam.d/$VERIFY_SVC"
install_pam=0
if command -v pamtester >/dev/null 2>&1; then
  echo "    verifying the staged stack as $me (type your password)"
  # authenticate + acct_mgmt is exactly what greetd asks of the stack;
  # open_session isn't tested because it really starts a keyring daemon.
  if sudo pamtester "$VERIFY_SVC" "$me" authenticate acct_mgmt; then
    echo "    verified — promoting to /etc/pam.d/greetd"
    install_pam=1
  else
    echo "!! pamtester FAILED. /etc/pam.d/greetd is untouched and greetd is NOT enabled." >&2
    exit 1
  fi
else
  echo "    !! pamtester isn't installed, so the stack can't be verified. A broken"
  echo "       stack means no graphical login; keep a TTY (Ctrl+Alt+F3) handy."
  ask_yn "Install the UNVERIFIED PAM stack anyway?" n && install_pam=1
  (( install_pam )) || echo "    skipped — /etc/pam.d/greetd left as it was."
fi
if (( install_pam )); then
  if [[ -f /etc/pam.d/greetd && ! -f /etc/pam.d/greetd.bak ]] && ! cmp -s "$HERE/greetd/pam/greetd" /etc/pam.d/greetd; then
    sudo cp /etc/pam.d/greetd /etc/pam.d/greetd.bak
    echo "    backed up the previous stack to /etc/pam.d/greetd.bak"
  fi
  sudo install -Dm644 "$HERE/greetd/pam/greetd" /etc/pam.d/greetd
fi

echo "==> greetd service drop-in (memory locking + restart policy)"
sudo install -Dm644 "$HERE/greetd/greetd.service.d/raingreet.conf" /etc/systemd/system/greetd.service.d/raingreet.conf
sudo systemctl daemon-reload
sudo systemctl reset-failed greetd 2>/dev/null || true

# Swap display managers. This only changes the next boot; the running session
# is untouched. The old one stays installed as the way back.
current="$(basename "$(readlink /etc/systemd/system/display-manager.service 2>/dev/null)" .service || true)"
if [[ -n $current && $current != greetd ]]; then
  echo "==> Disabling the current display manager: $current (still installed)"
  sudo systemctl disable "$current"
fi
echo "==> Enabling greetd"
sudo systemctl enable greetd

cat <<'EOF'

==> Done. raingreet is the login screen from the next boot.
    - Enter your username (remembered after the first login), Enter, then your
      password: each key is a drop on the glass. Esc on an empty password goes
      back to the username.
    - F2 picks the session (Niri, LifeBranch, ...). Ctrl+Alt+Delete reboots.
    - Logs: journalctl -t raingreet, and -t greetd-session for session startup.
    - Never restart greetd from inside a session it started: that ends the
      session. Reboot instead.
    - If it ever breaks, Ctrl+Alt+F3 to a TTY, log in, then either
        tuigreet:  sudo install -Dm644 ~/git/New-Niri-minimal-dots/rainglass/greetd/config-tuigreet.toml /etc/greetd/config.toml
                   sudo systemctl restart greetd
        old DM:    sudo systemctl disable greetd && sudo systemctl enable <old DM> && reboot
EOF
if [[ -n $current && $current != greetd ]]; then
  echo "    (the old display manager here is $current)"
fi
