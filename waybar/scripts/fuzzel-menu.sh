#!/usr/bin/env bash
#
# Shared fuzzel launcher for the waybar modules.
#
# fuzzel allows only one instance per Wayland session and prints
#   "failed to acquire lock: fuzzel already running?"
# when a second one is started. Waybar can invoke a module's on-click while
# another fuzzel menu (e.g. the Mod+Space launcher) is still open, so drop the
# duplicate request instead of spamming the log. stdin/stdout are inherited,
# so dmenu pipelines keep working.
if pgrep -x fuzzel >/dev/null 2>&1; then
    exit 0
fi

exec fuzzel "$@"
