# Themes

One colour palette for the whole desktop. Pick one and niri, waybar, fuzzel,
mako, kitty, alacritty, GTK, Qt, btop, cava, fish and the lock screen all
change together.

```sh
theme                    # picker in fuzzel (Mod+Shift+T); each pick applies at once, Esc closes
theme set gruvbox-dark   # apply one
theme random cyberpunk   # a random one, optionally from one family
theme list               # every palette, by family; * marks the one in use
theme show nord          # a palette's colours in the terminal
theme current
theme reload             # apply the current palette again (after editing a template)
theme check              # lint every palette and template
```

The script is `local/bin/theme`. It remembers the palette in use in
`~/.local/state/niri-theme`.

## How it works

Each app's own config includes one small generated file, and that file is the
only place its colours live:

| App | Generated file (gitignored) | Included by | Picks it up |
|---|---|---|---|
| niri | `niri/theme.kdl` | `config.kdl`, last, `optional=true` | at once (niri watches it) |
| waybar | `waybar/theme.css` | `@import` at the top of `style.css` | at once (SIGUSR2) |
| fuzzel | `fuzzel/theme.ini` | `include=` in `fuzzel.ini` | next launch |
| mako | `mako/theme` | `include=` at the top of `mako/config` | at once (`makoctl reload`) |
| kitty | `kitty/theme.conf` | `globinclude` in `kitty.conf` | at once (SIGUSR1) |
| alacritty | `alacritty/theme.toml` | `general.import` | at once (live reload) |
| GTK 3 and 4 | `gtk-3.0/theme.css`, `gtk-4.0/theme.css` | each `gtk.css` | each app's next start |
| Qt | `qt6ct/colors/theme.conf` | `color_scheme_path` in `qt6ct.conf` | each app's next start |
| btop | `btop/themes/current.theme` | `color_theme = "current"` | next start |
| cava | `cava/themes/current` | `theme = 'current'` | at once (SIGUSR2) |
| fish | `fish/conf.d/theme.fish` | sourced after `fish_frozen_theme.fish` | new shells |
| rainlock | `~/.config/rainlock/theme` | read before `~/.config/rainlock/config` | next lock |

So layout and fonts stay in each app's config, and only colours come from
here. The generated files are rendered from `templates/`, one per app.
`install.sh` writes them on a fresh install. The configs need them: fuzzel,
mako and cava refuse to start if theirs is missing, so run `theme reload` if
one ever goes.

`theme` also sets GTK's light or dark mode in gsettings: `color-scheme`
(libadwaita, browsers, Electron) and `gtk-theme` (adw-gtk3 or adw-gtk3-dark).
adw-gtk3 is the GTK3 theme that follows the colours in `theme.css`.

In the LifeBranch session, niri, kitty and the bar keep LifeBranch's own
colours; GTK, Qt and fish are shared, so they follow the palette there too.

## Palettes

80 palettes in ten families. Each family is a folder in `palettes/`; the id
is the file name.

### Originals

| Name | id | Variant | Look |
|---|---|---|---|
| Deep Navy | `deep-navy` | dark | Deep navy with sunset-orange accents: alacritty's old look |
| Glassy Frost | `glassy-frost` | dark | Near-black glass with Dracula-bright neon: kitty's old look |
| Sakura | `sakura` | dark | Cherry-blossom pink and lavender on plum: mako's old look |

### Mocha

| Name | id | Variant | Look |
|---|---|---|---|
| Café au Lait | `cafe-au-lait` | light | Milky coffee paper with dark-roast ink and a sage accent |
| Catppuccin Frappé | `catppuccin-frappe` | dark | The lightest dark Catppuccin flavour: muted, cool and low-glare |
| Catppuccin Latte | `catppuccin-latte` | light | Catppuccin's light flavour: milky whites with saturated accents |
| Catppuccin Macchiato | `catppuccin-macchiato` | dark | The mid-dark Catppuccin flavour, a touch softer than Mocha |
| Catppuccin Mocha | `catppuccin-mocha` | dark | Soothing pastel on deep mocha, the darkest Catppuccin flavour |
| Espresso | `espresso` | dark | Dark roast and crema: caramel, pistachio and raspberry on brown |
| Mocha Mousse | `mocha-mousse` | dark | Pantone's 2025 Mocha Mousse, with sage and latte on cocoa |

### Pastel

| Name | id | Variant | Look |
|---|---|---|---|
| Cotton Candy | `cotton-candy` | dark | Spun-sugar pink and baby blue, with a pastel rainbow along the bar |
| Pastel Lavender | `pastel-lavender` | dark | Lavender haze with powder blue and blush |
| Pastel Mint | `pastel-mint` | dark | Cool mint and sky over a deep green-grey |
| Pastel Peach | `pastel-peach` | light | Peach sorbet paper with coral, sea-glass and plum ink |
| Rosé Pine Dawn | `rose-pine-dawn` | light | Rosé Pine's morning: parchment, iris and pine |
| Rosé Pine Moon | `rose-pine-moon` | dark | Rosé Pine with a little more moonlight in the base |
| Rosé Pine | `rose-pine` | dark | All natural pine, faux fur and a bit of soho vibes |

### Rainbow

| Name | id | Variant | Look |
|---|---|---|---|
| Acid | `acid` | dark | Toxic lime, electric aqua and shock pink on swamp black |
| Candy Pop | `candy-pop` | dark | Bubblegum, lemon drop and blue raspberry on grape |
| Deep Sea | `deep-sea` | dark | Bioluminescence in the abyss: jade, cyan and jellyfish violet |
| Prism | `prism` | dark | Neon light split through glass, on midnight indigo |
| RGB Gamer | `rgb-gamer` | dark | The gaming-rig look: pure red, green and blue on black |
| Spectrum | `spectrum` | dark | Every hue at full strength on black; the bar runs red to violet |
| Sunset Strip | `sunset-strip` | dark | A vivid sunset, gold to violet, over a dusky plum sky |
| Ultraviolet | `ultraviolet` | dark | Blacklight purple with hot magenta and laser blue |

### Cyberpunk

| Name | id | Variant | Look |
|---|---|---|---|
| Blade Runner | `blade-runner` | dark | Smog-orange haze, teal neon and a pink hologram in the rain |
| Cyberpunk 2077 | `cyberpunk-2077` | dark | Night City HUD: warning yellow, netrunner cyan and arasaka red |
| Ghost in the Shell | `ghost-shell` | dark | Section 9 at night: thermoptic teal, data green and violet |
| Matrix | `matrix` | dark | Falling code: everything green, nothing else |
| Outrun | `outrun` | dark | Chrome sunset over a midnight highway: gold, coral, magenta, cyan |
| SynthWave '84 | `synthwave-84` | dark | Neon pink and cyan glowing over an 80s purple dusk |
| Tron Legacy | `tron-legacy` | dark | The Grid: light-cycle cyan, with Clu's orange for what's in focus |
| Vaporwave | `vaporwave` | dark | A E S T H E T I C: the classic five on a deep violet grid |

### Retro computers

| Name | id | Variant | Look |
|---|---|---|---|
| Amber Phosphor | `amber-phosphor` | dark | An amber CRT terminal, warm and easy on the eyes at 2 a.m. |
| Amiga Workbench | `amiga-workbench` | dark | Workbench 1.3: white on that blue, orange for what matters |
| CGA | `cga` | dark | Four colours and no apologies: cyan, magenta and white on black |
| Commodore 64 | `commodore-64` | dark | READY. Light blue on the C64's blue screen, with its yellow and green |
| Green Phosphor | `green-phosphor` | dark | A green-screen CRT: every colour is a brightness of P1 green |
| Macintosh 1984 | `macintosh-1984` | light | Black on white, 512x342, and not a single colour |
| MS-DOS | `ms-dos` | dark | VGA text mode: white on blue for the desktop, a black DOS prompt in the terminal |
| PC-98 | `pc-98` | dark | Dithered midnight blues with pink and teal, like a PC-98 adventure game |
| Teletext | `teletext` | dark | Page 100: yellow headlines, cyan text and the full eight-colour band |
| Windows 95 | `windows-95` | light | Start me up: button-face grey, navy highlights and the teal desktop |
| ZX Spectrum | `zx-spectrum` | dark | Ink and paper on black, under the red-yellow-green-cyan stripe |

### Retro consoles

| Name | id | Variant | Look |
|---|---|---|---|
| Atari 2600 | `atari-2600` | dark | Woodgrain veneer, black ribs and Atari orange |
| Dreamcast | `dreamcast` | light | White console, orange swirl, and the A, B, X, Y buttons |
| Famicom | `famicom` | light | Cream and burgundy with gold trim, like the original Famicom |
| Game Boy | `game-boy` | light | The DMG's four greens, dark pixels on the pale screen |
| GameCube | `gamecube` | dark | Indigo plastic with the big green A button, red B and yellow C |
| Mega Drive | `mega-drive` | dark | Black plastic, red and gold 16-BIT lettering, Sega blue |
| Nintendo 64 | `n64` | dark | Charcoal with the logo's blue, red, green and yellow |
| NES | `nes` | dark | Grey console, black stripe, red logo, and the 8-bit palette |
| PICO-8 | `pico-8` | dark | The fantasy console's 16 colours on its dark-blue screen |
| PlayStation | `playstation` | dark | Grey console with triangle, circle, cross and square along the bar |
| SNES | `snes` | dark | Lavender-grey plastic and the red, yellow, blue, green buttons |
| Virtual Boy | `virtual-boy` | dark | Red on black and nothing else, as Nintendo intended in 1995 |

### Classics

| Name | id | Variant | Look |
|---|---|---|---|
| Ayu Dark | `ayu-dark` | dark | Ayu's near-black with marigold, sky and sage |
| Dracula | `dracula` | dark | The dark theme for everything: purple, pink and neon on deep grey |
| Gruvbox Dark | `gruvbox-dark` | dark | Retro groove: warm, earthy, high-contrast without the glare |
| Gruvbox Light | `gruvbox-light` | light | Gruvbox on its warm parchment background |
| Monokai | `monokai` | dark | The Sublime Text classic: hot pink, lime and sky on olive-black |
| Nordfox | `nordfox` | dark | Nightfox's take on Nord, as in this setup's Neovim |
| Nord | `nord` | dark | An arctic, north-bluish palette: Polar Night, Snow Storm, Frost, Aurora |
| One Dark | `one-dark` | dark | Atom's classic: balanced, quiet, endlessly ported |
| Oxocarbon | `oxocarbon` | dark | IBM Carbon greys with pink, teal and violet |
| Solarized Dark | `solarized-dark` | dark | Ethan Schoonover's precision palette on its deep teal base |
| Solarized Light | `solarized-light` | light | Solarized's same accents on its cream base |
| Tokyo Night | `tokyo-night` | dark | Downtown Tokyo's lights at night: soft blues and violets on ink |

### Nature

| Name | id | Variant | Look |
|---|---|---|---|
| Autumn | `autumn` | dark | Pumpkin, mustard and maple red on bark brown |
| Desert Dune | `desert-dune` | light | Sun-bleached sand, terracotta and an oasis of teal |
| Everforest | `everforest` | dark | Green forest calm: warm sage and moss, easy on the eyes |
| Glacier | `glacier` | dark | Ice blue and glacial teal under a violet aurora |
| Kanagawa | `kanagawa` | dark | The Great Wave: sumi ink, fuji white, crystal blue and autumn red |
| Moss | `moss` | dark | Damp stone and moss, with lichen gold and bark |

### Minimal

| Name | id | Variant | Look |
|---|---|---|---|
| E-Ink | `e-ink` | light | An e-reader screen: soft paper grey and ink, barely any colour |
| Mono | `mono` | dark | Greys with white for focus; colour only where it means something |
| Noir | `noir` | dark | Black and white film with one red: focus, errors, nothing else |
| Paper | `paper` | light | Warm notebook paper with fountain-pen blue ink |
| Slate | `slate` | dark | Cool grey stone with a calm blue accent |
| Sumi-e | `sumi-e` | light | Ink wash on washi paper, with a vermilion seal |

## Writing a palette

A palette is `palettes/<family>/<id>.toml`: lines of `key = "value"`, with
`#` comments. Only these are required:

```toml
name    = "My Palette"
variant = "dark"            # or "light"
about   = "One line for theme show"   # optional
bg      = "#1e1e2e"
fg      = "#cdd6f4"
accent  = "#b4befe"         # focus: active workspace, selection, focus ring
color0  = "#45475a"         # terminal colours 0-7: black red green yellow
color1  = "#f38ba8"         #   blue magenta cyan white
# ... color2 to color7
```

Everything else is worked out from those, and any of it can be set to
override that:

| Key | Default | Used for |
|---|---|---|
| `crust` | bg darkened | the darkest layer: bar pills, tooltips |
| `surface`, `overlay`, `surface2` | bg moved 10%, 20%, 30% towards fg | selections, borders, headerbars |
| `view` | between bg and crust (lighter than bg on light palettes) | GTK and Qt content areas |
| `subtext`, `muted` | fg moved 15%, 45% towards bg | secondary and dimmed text |
| `accent2`, `accent3` | color6, color5 | bar text and links; hover and visited |
| `red` `green` `yellow` `blue` `magenta` `cyan` | color1 to color6 | |
| `orange` | between red and yellow | |
| `urgent`, `warn`, `ok`, `info` | red, orange, green, blue | battery, notifications, errors |
| `color8` to `color15` | color0 to color7 lightened (darkened on light palettes) | |
| `term_bg`, `term_fg` | bg, fg | terminal background and text, if they differ (MS-DOS, Windows 95) |
| `selection`, `selection_fg` | accent blended into the terminal bg; term_fg | |
| `cursor`, `cursor_text` | accent, term_bg | |
| `border`, `border_inactive` | accent, overlay | niri focus ring and border |
| `bar_bg` | crust | waybar pills |
| `band1` to `band8` | all accent2 | the colours of the right-hand bar modules, in order; set a few and they repeat |
| `on_accent`, `on_urgent`, `on_red`, ... | bg or fg, whichever reads better | text drawn on that colour |
| `lock_text` | the lighter of fg and bg | lock screen text |

Run `theme check` after writing one. It flags text that's hard to read (by
WCAG contrast ratio) and notes terminal colours that are faint on the
background. Some retro palettes keep those on purpose, like the VGA dark blue.
`theme show <id>` previews it.

## Templates

`templates/<app>` is the file the app includes, with `{{key}}` for colours.
Each key also comes as `{{key.hex}}` (no `#`) and `{{key.rgb}}`
(`30, 30, 46`). Other keys: `{{name}}`, `{{id}}`, `{{family}}`,
`{{variant}}`, `{{dark}}` (true or false), `{{gtk_theme}}`,
`{{color_scheme}}`, and `{{gradient1}}` to `{{gradient8}}`, which run through
the bands (or accent2, accent and accent3) for cava and btop. An unknown key
is an error, so a typo never writes half a theme.

To theme another app: add a template, add a line to `TARGETS` in
`local/bin/theme`, include the generated file from the app's config, and
gitignore it.

Not themed yet: tmux, starship, Neovim and Zed keep their own colours.

## Credits

The palettes named after a project use that project's published colours:
Catppuccin, Rosé Pine, Nord, Gruvbox, Dracula, Tokyo Night, One Dark,
Solarized, Monokai, Ayu, Oxocarbon, Nightfox, Everforest, Kanagawa and
SynthWave '84, credited in each file. The retro palettes follow the hardware's
own colours where it had a fixed set: Pepto's C64 palette, the ZX Spectrum,
CGA/VGA, Workbench 1.3, Teletext, the Game Boy and Virtual Boy screens, the
NES 2C02 and PICO-8. The rest are original to this repo.
