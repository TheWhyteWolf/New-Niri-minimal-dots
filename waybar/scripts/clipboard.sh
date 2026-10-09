#!/bin/bash

# Clipboard manager script using cliphist

# Shared wrapper: skips the request when another fuzzel instance is already open.
FUZZEL="$HOME/.config/waybar/scripts/fuzzel-menu.sh"

show_clipboard() {
    local selection
    selection=$(cliphist list | "$FUZZEL" --dmenu --prompt="Clipboard: " --width=60)
    # fuzzel returns nothing when the picker is cancelled; feeding that to
    # `cliphist decode` fails with "input not prefixed with id".
    [ -n "$selection" ] || return 0
    printf '%s' "$selection" | cliphist decode | wl-copy
}

clear_clipboard() {
    cliphist wipe
    notify-send "Clipboard" "History cleared"
}

case "$1" in
    --show)
        show_clipboard
        ;;
    --clear)
        clear_clipboard
        ;;
    *)
        echo "Usage: $0 {--show|--clear}"
        exit 1
        ;;
esac
