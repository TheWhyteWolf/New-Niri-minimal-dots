# rainglass

The lock screen and login screen for this setup: your wallpaper seen through
a rainy window. Drops sit on the glass and run down it, wiping clear trails
through the fog; each one is a small lens that shows the picture behind it
upside down, the way real drops do.

| crate | what it is |
|---|---|
| `rainglass` | the renderer both use: wallpaper fitting and frosting, the rain shader, text, fractional scaling |
| `rainlock` | the lock screen (ext-session-lock) |
| `raingreet` | the login screen (a greetd greeter, run under cage) |
| `lifefont` | glyph rasterizer, from LifeBranch |

While you type your password, each key lands a drop. Backspace wipes the
newest one and Esc wipes them all. The drops are gone within two seconds, so
nobody can count them later. The rain all but stops while the password is
checked. A wrong password flashes the glass red; the right one wipes the
fog away.

## Install

```sh
bash rainglass/install.sh          # the lock screen
bash rainglass/greeter-install.sh  # the login screen (replaces the display manager)
```

`install.sh` builds rainlock, links it into `~/.local/bin` and installs
`/etc/pam.d/rainlock`. `niri-lock/lock.sh` uses rainlock once it is there, and
gtklock until then, or if rainlock ever fails to lock. Mod+Alt+Escape locks;
so does 10 minutes idle and going to sleep. If the lock ever wedges,
Mod+Shift+Alt+Escape hands it to gtklock.

`greeter-install.sh` switches the login screen to greetd + raingreet and
keeps tuigreet as a fallback. The picture is a copy of the desktop's
wallpaper (or `--image PATH`), because the greeter user can't read your home.
The script's header has the way back. F2 picks the session (Niri, LifeBranch,
...).

## Settings

- rainlock reads `~/.config/rainlock/config`; `rainlock --help` lists the keys.
  Its text colours come from the desktop's palette (`theme`, see
  `themes/README.md`), via `~/.config/rainlock/theme`; a colour set in the
  config wins over it.
- raingreet reads `/etc/raingreet/config`; `raingreet --help` lists the keys.

For example, `density=0.4` for lighter rain, `image=~/Pictures/wallpapers/x.jpg`
for a fixed picture, `fps=20`, or `still` to draw one frame with no animation.
By default the lock shows the wallpaper the desktop is showing, which
`niri/autostart.sh` and `local/bin/set-wallpaper` record in
`~/.local/state/niri-wallpaper`.

To see the look without locking anything:

```sh
cargo run --release --manifest-path rainglass/Cargo.toml --example preview -- \
  --image ~/Pictures/wallpapers/rogue.jpg --clock --splash --png /tmp/rain.png
```

`--bench 120` adds the GPU cost per frame. On this laptop's Intel HD 630 it
is about 6 ms at 1920x1080, which keeps 30 fps at under a fifth of the GPU.
The rain stops when the screen turns off.

To try the greeter as a normal window, use a debug build. Its stub accepts the
password `ok`; the stub isn't in release builds.

```sh
cargo build --manifest-path rainglass/Cargo.toml -p raingreet
RAINGREET_TEST_NO_IPC=1 rainglass/target/debug/raingreet --image ~/Pictures/wallpapers/rogue.jpg --remember ""
```

## Where it comes from

Everything security-related is LifeBranch's lifelock and lifegreet, unchanged
except for names:

- rainlock: the separate PAM process, the locked password buffer, `-f`, the
  hardening, the frame watchdog and monitor hotplug.
- raingreet: the greetd connection, the session list and the phase machine.

LifeBranch's lifelock modelled its security code on swaylock; `NOTICE` has
the details. The GPU setup is lifewall's. The rain shader is new, written for
this project; it doesn't use the well-known "Heartfelt" Shadertoy, whose
licence forbids commercial use.

Licence: GPL-3.0-or-later (`LICENSE`), because of the LifeBranch code it
builds on. The rest of the dotfiles repo stays MIT.
