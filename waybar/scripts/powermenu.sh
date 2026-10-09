#!/bin/bash

# Power menu script using fuzzel

FUZZEL="$HOME/.config/waybar/scripts/fuzzel-menu.sh"

OPTIONS="⏻ Shutdown\n⏾ Reboot\n󰒲 Suspend\n󰍃 Logout\n󰷛 Lock"

CHOICE=$(echo -e "$OPTIONS" | "$FUZZEL" --dmenu --prompt="Power Menu: " --width=20)

case "$CHOICE" in
    *"Shutdown")
        systemctl poweroff
        ;;
    *"Reboot")
        systemctl reboot
        ;;
    *"Suspend")
        systemctl suspend
        ;;
    *"Logout")
        niri msg action quit
        ;;
    *"Lock")
        niri-lock
        ;;
esac
