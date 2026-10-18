#!/usr/bin/env bash
# Lock the session: rainlock (the wallpaper through a rainy window; see
# rainglass/), or gtklock if rainlock isn't built yet. Both fork once the
# session is locked (-f / -d), so swayidle's before-sleep waits for the lock
# and nothing else.

rainlock="$HOME/.local/bin/rainlock"
if [ -x "$rainlock" ]; then
  # Already locked (a second Mod+Alt+Escape, or idle on top of a manual lock).
  pgrep -x rainlock >/dev/null && exit 0
  # -f returns 0 only once the session is locked. Anything else (no PAM file
  # yet, no Wayland, a crash) falls through to gtklock: never leave it open.
  "$rainlock" -f && exit 0
  echo "lock.sh: rainlock failed to lock; falling back to gtklock" >&2
fi

# Kill previous locks (prevents crash loop)
pkill gtklock 2>/dev/null

# Start gtklock cleanly
gtklock -d \
  --config "$HOME/.config/niri-lock/config.ini"
