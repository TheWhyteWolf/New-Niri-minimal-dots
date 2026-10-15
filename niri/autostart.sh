#!/bin/sh

# Cursor
export XCURSOR_THEME="Bibata-Modern-Classic"
export XCURSOR_SIZE=24
export XCURSOR_PATH="$HOME/.config/niri:$HOME/.icons:$HOME/.local/share/icons:/usr/share/icons"

# CRITICAL ENV (FIXED)
export XDG_CURRENT_DESKTOP=niri
export XDG_SESSION_DESKTOP=niri
export XDG_SESSION_TYPE=wayland
export GTK_USE_PORTAL=1

# Export to DBus (REQUIRED)
dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_CURRENT_DESKTOP &

sh -c 'while ! busctl --user status org.gnome.Mutter.ScreenCast >/dev/null 2>&1; do sleep 0.2; done; pkill -f xdg-desktop-portal-gnome' &

# Wallpaper: choose a different image at random each session.
wallpaper_dir="$HOME/Pictures/wallpapers"
wallpaper=$(find "$wallpaper_dir" -maxdepth 1 -type f \
  \( -iname '*.jpg' -o -iname '*.jpeg' -o -iname '*.png' -o -iname '*.webp' \) \
  -print 2>/dev/null | shuf -n 1)

if [ -n "$wallpaper" ]; then
  swaybg -i "$wallpaper" -m fill &
fi

# Notifications
mako &

# Clipboard
wl-paste --watch cliphist store &

# Bar
waybar &

# Polkit
/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1 &
