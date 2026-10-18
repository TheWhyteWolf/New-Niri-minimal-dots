#!/bin/bash
# ╔═══════════════════════════════════════════════════════════════╗
# ║  Niri Dotfiles - Installation Script                        ║
# ║  Fresh Arch → Clone → Run → Working Desktop                 ║
# ╚═══════════════════════════════════════════════════════════════╝

set -euo pipefail

DOTFILES_DIR="$(cd "$(dirname "$0")" && pwd)"
CONFIG_DIR="$DOTFILES_DIR"

# ── Colors ─────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log()   { echo -e "${GREEN}[+]${NC} $1"; }
warn()  { echo -e "${YELLOW}[!]${NC} $1"; }
error() { echo -e "${RED}[x]${NC} $1"; exit 1; }
info()  { echo -e "${BLUE}[i]${NC} $1"; }

# ── Preflight Checks ──────────────────────────────────────────
if ! command -v pacman &>/dev/null; then
    error "This script is designed for Arch Linux (pacman not found)"
fi

if [ "$(id -u)" -eq 0 ]; then
    error "Do not run this script as root"
fi

echo -e "${BLUE}"
echo "  ╔═══════════════════════════════════════╗"
echo "  ║     Niri Dotfiles Installer           ║"
echo "  ╚═══════════════════════════════════════╝"
echo -e "${NC}"

# ── Install Packages ───────────────────────────────────────────
log "Installing packages from packages.txt..."
if [ -f "$DOTFILES_DIR/packages.txt" ]; then
    mapfile -t packages < <(grep -vE '^[[:space:]]*(#|$)' "$DOTFILES_DIR/packages.txt")
    sudo pacman -S --needed --noconfirm "${packages[@]}"
else
    warn "packages.txt not found, skipping package installation"
fi

# ── Install yay (AUR helper) ──────────────────────────────────
if ! command -v yay &>/dev/null; then
    log "Installing yay (AUR helper)..."
    git clone https://aur.archlinux.org/yay-bin.git /tmp/yay-bin
    (cd /tmp/yay-bin && makepkg -si --noconfirm)
    rm -rf /tmp/yay-bin
fi

# ── Install AUR packages ──────────────────────────────────────
log "Installing AUR packages..."
yay -S --needed --noconfirm \
    niri-git \
    niri-settings-git \
    niri-utils \
    bibata-cursor-theme-bin \
    brave-bin \
    vesktop-bin \
    sddm-sugar-candy-git \
    pomoru \
    freesmlauncher-bin \
    dracula-gtk-theme \
    neofetch \
    pipes.sh \
    tty-clock \
    femboysay \
    2>/dev/null || warn "Some AUR packages may have failed"

# ── Create Directories ────────────────────────────────────────
log "Creating directories..."
mkdir -p "$HOME/.config"
mkdir -p "$HOME/.local/bin"
mkdir -p "$HOME/Pictures/wallpapers"
mkdir -p "$HOME/Pictures/Screenshots"
mkdir -p "$HOME/.config/xdg-desktop-portal"

# ── Symlink Configurations ────────────────────────────────────
log "Linking configurations to ~/.config/..."

link_config() {
    local src="$1"
    local dest="$HOME/.config/$2"
    local name="$2"
    local file

    if [ -e "$dest" ] && [ ! -L "$dest" ]; then
        warn "Backing up existing $name → $name.bak"
        mv "$dest" "$dest.bak.$(date +%s)"
    fi

    if grep -RIlq '__HOME__' "$src"; then
        # Copy configs that need an absolute home path. Replacing tokens in a
        # symlinked config would also edit the tracked files in this checkout.
        [ ! -L "$dest" ] || unlink "$dest"
        cp -a "$src" "$dest"
        while IFS= read -r -d '' file; do
            sed -i "s|__HOME__|$HOME|g" "$file"
        done < <(grep -RIlZ '__HOME__' "$dest")
        log "Installed $name with home paths"
    else
        ln -sfn "$src" "$dest"
        log "Linked $name"
    fi
}

# Link all config directories
for dir in "$CONFIG_DIR"/*/; do
    app_name=$(basename "$dir")

    # Skip directories that aren't configs
    case "$app_name" in
        .git|Screenshots|wallpapers|local|nvim|xdg-desktop-portal|rainglass|themes) continue ;;
    esac

    link_config "$dir" "$app_name"
done

# ── Link Local Bin Scripts ────────────────────────────────────
if [ -d "$CONFIG_DIR/local/bin" ]; then
    mkdir -p "$HOME/.local/bin"
    for script in "$CONFIG_DIR/local/bin/"*; do
        [ -f "$script" ] || continue
        script_name=$(basename "$script")
        ln -sfn "$script" "$HOME/.local/bin/$script_name"
    done
    log "Linked local/bin scripts"
fi

# Brave reads its flags from a file at the top of ~/.config, not a directory.
[ -f "$CONFIG_DIR/brave-flags.conf" ] && ln -sfn "$CONFIG_DIR/brave-flags.conf" "$HOME/.config/brave-flags.conf"

# Colours: waybar, mako, cava and the rest include files that `theme`
# generates, so write them now (the palette in use, else Catppuccin Mocha).
log "Applying the colour theme..."
"$CONFIG_DIR/local/bin/theme" reload || warn "theme reload failed; run: theme reload"

# ── Install wallpapers ────────────────────────────────────────
log "Installing wallpapers..."
if [ -d "$CONFIG_DIR/wallpapers" ]; then
    mkdir -p "$HOME/Pictures/wallpapers"
    for wallpaper in "$CONFIG_DIR/wallpapers"/*; do
        [ -f "$wallpaper" ] || continue
        name=$(basename "$wallpaper")
        [ "$name" = ".gitkeep" ] && continue
        if [ ! -f "$HOME/Pictures/wallpapers/$name" ]; then
            cp "$wallpaper" "$HOME/Pictures/wallpapers/$name"
            log "Installed wallpaper: $name"
        fi
    done
fi

# ── Install MIME associations ─────────────────────────────────
log "Installing MIME associations..."
if [ -f "$CONFIG_DIR/mimeapps.list" ]; then
    cp "$CONFIG_DIR/mimeapps.list" "$HOME/.config/mimeapps.list"
    log "Installed mimeapps.list"
fi

# ── Install portal configuration ─────────────────────────────
log "Installing portal configuration..."
if [ -d "$CONFIG_DIR/xdg-desktop-portal" ]; then
    mkdir -p "$HOME/.config/xdg-desktop-portal"
    for conf in "$CONFIG_DIR/xdg-desktop-portal"/*.conf; do
        [ -f "$conf" ] || continue
        cp "$conf" "$HOME/.config/xdg-desktop-portal/$(basename "$conf")"
        log "Installed portal config: $(basename "$conf")"
    done
fi

# ── Set Fish as Default Shell ──────────────────────────────────
if [ "$SHELL" != "/usr/bin/fish" ]; then
    log "Setting fish as default shell..."
    chsh -s /usr/bin/fish
    info "Shell changed to fish. Will take effect on next login."
fi

# ── Enable Services ────────────────────────────────────────────
log "Enabling services..."
sudo systemctl enable sddm 2>/dev/null || true

# ── Make Scripts Executable ────────────────────────────────────
log "Setting script permissions..."
find "$CONFIG_DIR" -name "*.sh" -exec chmod +x {} \; 2>/dev/null || true
find "$CONFIG_DIR/local/bin" -type f -exec chmod +x {} \; 2>/dev/null || true

# ── Done ───────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}╔═══════════════════════════════════════════╗"
echo -e "║     Installation Complete!                ║"
echo -e "╚═══════════════════════════════════════════╝${NC}"
echo ""
echo -e "  ${YELLOW}Next steps:${NC}"
echo -e "  1. Reboot your system"
echo -e "  2. Add wallpapers to ~/Pictures/wallpapers/"
echo -e "  3. Enjoy your new desktop!"
echo ""
echo -e "  ${BLUE}Note:${NC} If using SDDM, restart it with:"
echo -e "  sudo systemctl restart sddm"
echo ""
