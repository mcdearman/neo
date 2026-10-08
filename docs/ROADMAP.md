# Neo roadmap

From a toolkit and a set of apps, to a desktop session on an existing distro, to a Neo distribution.

Each stage ships something usable on its own, and each builds on the last without throwing work away. The shell parts of the desktop (panel, launcher, notifications) are ordinary Wayland clients, so the ones built for someone else's compositor in stage 2 also run on Neo's own compositor in stage 3.

| Stage | What people get | Runs on |
|---|---|---|
| 1. Apps | Neo apps they can install next to GNOME or KDE | Any Linux desktop, plus macOS and Windows |
| 2. Session | "Neo" to pick at the login screen | An existing distro such as Fedora or Arch |
| 3. Compositor | Neo draws the whole screen: glass blur, shadows and animation for every window | Same as stage 2 |
| 4. Distribution | A Neo ISO: install, update and roll back | Its own images, built on an existing base |

## Where things stand

Done:

- **Toolkit:** `neo-theme`, `armature-render` and `neo`.
  - Widgets, layout, focus, animation, glass windows and a headless test harness.
  - Background-thread messages (`Proxy`) and layout-time messages (`Cx::defer`).
- **Apps:** Files, Terminal, Settings, System Monitor, NeoCal and NeoCode.
  - They share one appearance file, and every app restyles live when Settings changes it.
- **Linux integration:** `.desktop` entries, Wayland app IDs and `dist/install.sh`.

Not yet done: nothing has run on a real Linux session. Everything type-checks for Linux, but it has only been run on macOS.

## Stage 1: apps that run on any Linux desktop

The goal is that someone on Fedora GNOME or Arch KDE installs the Neo apps and uses them every day.

### 1.1 Run it on Linux

- Set up a Linux development machine (see [Development machine](#development-machine)).
- Run every app on GNOME (Wayland), KDE Plasma (Wayland, where the blur protocol applies) and an X11 session. Fix what breaks, such as keyboard layouts, HiDPI and fractional scaling, IME, clipboard and window dragging.
- Add CI that builds and tests on Linux. The harness needs a GPU adapter; Mesa's llvmpipe works in CI.

### 1.2 Toolkit gaps the apps need

In rough priority order:

1. **Overlays:** menus, context menus, popovers, tooltips and modal dialogs. These unblock right-click in Files, a tab menu in Terminal and confirmation dialogs.
2. **Virtualised lists:** build only the visible rows. Files currently caps a folder at 800 rows and System Monitor at 150 processes.
3. **Images:** a textured-quad primitive in `armature-render`, with decoding off the main thread. This unblocks thumbnails, an image viewer, album art and wallpapers.
4. **Keyboard:**
   - Missing keys: F1 to F12, Insert and the numeric keypad.
   - IME pre-edit, beyond commit-only text.
   - Global shortcut handling that doesn't depend on focus.
5. **Accessibility:** expose the widget tree to screen readers through AccessKit (AT-SPI on Linux).
6. **Text:**
   - Italic and bold faces for the monospace font.
   - Better fallback for CJK and emoji, so Terminal columns stay aligned.
   - Selectable text labels.
7. **Drag and drop:** within an app first, then between apps through the Wayland data device.
8. **Async tasks:** a small helper on top of `Proxy`, so apps can run work such as a folder copy, a network request or a thumbnail without blocking.

### 1.3 Grow the app set

Next apps, most useful first:

- **Text Editor:** NeoCode accepts a file, not just a folder, and becomes the default for `text/plain`.
- **Image Viewer**, once images exist.
- **Music:** the gallery's player made real, with `symphonia` for decoding, PipeWire output and MPRIS controls.
- **Clock and Calendar:** alarms, timers and a month view. The panel reuses the calendar later.
- **Screenshot:** through the xdg-desktop-portal Screenshot API.
- **Archive Manager**, **Document Viewer** (PDF) and **Notes**.
- **Settings pages:**
  - Displays, Sound, Network, Bluetooth, Power, Keyboard, Mouse and Users.
  - Most of these talk to system services over D-Bus (NetworkManager, BlueZ, UPower, PipeWire, AccountsService) through `zbus`.
  - Some only work fully in a Neo session (stage 2), so each page detects what it can control.

### 1.4 Ship

- Packages: an AUR `neo-apps-git` package, a Fedora COPR repository and eventually Flathub. Each depends on `ffmpeg`, the one program Neo relies on and does not build (`xtask/src/deps.rs` has the list).
  - The Terminal and Files need broad file-system access, which fits native packages better than Flatpak.
- Replace the stock `Icon=` names in the `.desktop` files with Neo's own app icons.

Stage 1 is done when the apps run on GNOME and KDE on Wayland without Linux-specific bugs, install from a package, and the 1.2 items marked as blockers are in.

## Stage 2: a Neo session on an existing distro

The goal is a "Neo" entry at the login screen that starts a complete desktop. Two ways to get there:

| | A. Neo shell on an existing compositor | B. Neo's own compositor |
|---|---|---|
| What we build | Panel, launcher, notifications and the rest as Wayland clients | All of A, plus window management, rendering, input and output |
| Compositor | labwc (stacking, wlroots) or niri (scrolling tiling, Smithay) | `neo-compositor` on Smithay |
| Glass blur behind windows | No: labwc and niri don't blur | Yes, with the same look as in-window glass |
| Rounded corners and shadows on every window | Partly: Neo apps draw their own; other apps get the compositor's | Yes |
| Size of the work | Weeks | Months |
| What carries over | Every shell component | — |

**Recommendation:** do A first. It gives a daily-driver session soon, and it tests the shell components against a mature compositor. Start B (stage 3) alongside it. The shell components move over unchanged, because both compositors speak the same protocols.

### 2.1 Framework support for shell surfaces

winit cannot create panels, docks or lock screens. These need the `wlr-layer-shell` and `ext-session-lock` protocols.

- Add a second backend to `neo` built on `smithay-client-toolkit`, used only for these surfaces.
- `armature-render` already draws to any raw window handle, so only the shell's event and window plumbing is new.
- Apps don't change.

### 2.2 Shell components

Each is a small Neo program:

| Component | Protocols and services |
|---|---|
| **Panel**: app menu, clock and calendar, status icons (network, volume, battery), tray | layer-shell; `ext-foreign-toplevel-list` or `wlr-foreign-toplevel-management` for the task list; `ext-workspace` for workspaces; StatusNotifierItem over D-Bus for the tray |
| **Launcher**: app grid and search | Reads `.desktop` files; layer-shell overlay |
| **Notifications**: popups and a history | Implements `org.freedesktop.Notifications` |
| **OSD** for volume and brightness | PipeWire (WirePlumber), logind or backlight |
| **Lock screen** | `ext-session-lock-v1`, PAM |
| **Polkit agent**: the admin password dialog | `org.freedesktop.PolicyKit1.AuthenticationAgent` |
| **Portal backend**, `xdg-desktop-portal-neo` | File chooser, Screenshot, and Settings. Publishing `color-scheme` and `accent-color` makes GTK, libadwaita, Qt and Flatpak apps follow Neo's dark mode and accent. |
| **Settings daemon** | Applies fonts, cursor theme, keyboard layouts, display layout (`wlr-output-management`) and idle and power policy |
| **Greeter**: the login screen | A `greetd` greeter written with Neo |

### 2.3 Session plumbing

- `neo-session` starts the compositor and shell components, restarts any that crash, and sets up the environment:
  - `XDG_CURRENT_DESKTOP=Neo`
  - portals
  - `systemd --user` targets
- `/usr/share/wayland-sessions/neo.desktop` makes the session appear in GDM, SDDM or greetd.
- Default config for the chosen compositor: keybindings, window rules and server-side decorations that look like Neo's title bar.
- Theme non-Neo apps to match:
  - libadwaita through the portal.
  - GTK 3 through a Neo GTK theme, generated from `neo-theme`'s tokens.
  - Qt through a matching Kvantum theme or `qt6ct` colours.
  - Cursor and icon themes.

### 2.4 Target distro for development

Develop against one distro, then package for others.

- **Arch** is the quickest to iterate on: rolling Wayland stack, and AUR packages with no review queue.
- **Fedora** is the most likely base for the distribution in stage 4, and has current Mesa and portals. On Apple Silicon it is also the only well-supported option, as Fedora Asahi Remix.

Pick one according to the development machine.

Stage 2 is done when someone can log into Neo on a fresh install, use it for a normal day with a browser, the Neo apps and a Flatpak, and nothing needs a terminal to fix.

## Stage 3: Neo's own compositor

`neo-compositor` is built on Smithay, the Rust compositor library behind niri and COSMIC.

- **Rendering:** composite with `armature-render`.
  - Blur behind every translucent window, reusing the dual-Kawase pass the toolkit already has.
  - Rounded corners and soft shadows for every window, not just Neo apps.
  - Open, close, minimise and workspace animations that follow the reduced-motion setting.
- **Protocols:**
  - xdg-shell and xdg-decoration: Neo draws server-side title bars that match the client-side ones.
  - layer-shell and session-lock, so the stage 2 shell runs unchanged.
  - foreign-toplevel, workspaces and screencopy (`ext-image-copy-capture`), for the portal's screen sharing.
  - idle-inhibit, fractional-scale, viewporter, presentation-time, primary selection and data control.
  - A blur protocol, so apps can ask for blur the way KDE's protocol works.
- **Backends:**
  - The winit backend, for running nested in a window during development. This works on macOS through a Linux VM, but properly only on Linux.
  - DRM/KMS with libinput and libseat, for real sessions.
- **XWayland:** through `xwayland-satellite`, as niri does, so X11 apps work without X11 code in the compositor.
- **Window management** is a decision to make first (see [Decisions](#decisions)):
  - stacking, as in GNOME, KDE and macOS
  - tiling, as in Sway
  - scrolling, as in niri

  It shapes the panel, the keybindings and the overview.
- **Reliability:** a crash must not lose the session. Keep compositor state small, and let the shell components reconnect after a restart.

Stage 3 is done when `neo-compositor` replaces the stage 2 compositor as the default and passes a daily-use test on AMD, Intel and, if supported, NVIDIA GPUs.

## Stage 4: a Neo distribution

The recommendation is **not** to build a distribution from scratch. Build Neo images on an existing base, the way Bluefin and Bazzite build on Fedora and Pop!_OS builds on Ubuntu.

| Base | How | For | Against |
|---|---|---|---|
| **Fedora Atomic (bootc)**, recommended | A `Containerfile` `FROM` Fedora's bootc image, adding Neo's packages and settings. CI builds images and ISOs. | Atomic updates with rollback, little maintenance, current Mesa and kernel, Asahi support | Changes to the base system go through image rebuilds, not `dnf install` |
| Arch-based | `archiso` and the Calamares installer | Fastest to follow upstream | Neo would own breakage from rolling updates |
| NixOS module | `services.desktopManager.neo.enable = true;` | Reproducible, easy to try | Smaller audience |
| Ubuntu or Debian-based | Neo packages on top of Ubuntu LTS | Largest user base, stable | Older graphics stack |

What a Neo distribution adds beyond the session:

- **First-run setup:** language, keyboard, network, user account, appearance and privacy. It's a Neo app run by the greeter.
- **Installer:** `bootc install` or Anaconda for Fedora; Calamares for others.
- **Software:** a Neo app store front-end over Flatpak and Flathub, with AppStream metadata. System updates are shown as one "Update and restart" through bootc.
- **Hardware:** firmware through fwupd, printers through CUPS and IPP Everywhere, and NVIDIA drivers through prebuilt kernel modules.
- **Branding:** Plymouth boot splash, bootloader theme, wallpapers, sounds and default settings.
- **Infrastructure:** signed image builds in CI, update channels (stable, beta), a website with install docs, a bug tracker, and opt-in crash reports.

## Development machine

The Mac used so far, an M2 Pro, is supported by **Fedora Asahi Remix**, which has working OpenGL and Vulkan drivers.

- **Dual-boot Asahi.** Stages 1 to 3 can be developed and run natively on the same machine, including DRM/KMS compositor work.
- **A Linux VM** (UTM or Parallels). Good enough for app testing in stage 1. GPU acceleration in VMs is limited, so it's a poor fit for compositor work.
- **A second x86 PC with an AMD GPU.** The most common hardware Neo would ship on, and the best-supported by Mesa. Worth having by stage 3 regardless.

## Decisions

These shape later work, so settle them before the stage that needs them:

1. **Development machine (now):** Asahi dual-boot, VM, or a separate PC.
2. **Window management model (before stage 3; ideally before the stage 2 panel):** stacking, tiling or scrolling. This also decides which compositor stage 2A uses: labwc for stacking, niri for scrolling.
3. **Base distro (before stage 2.4):** Fedora or Arch for development. This likely decides the stage 4 base.
4. **Licence (before making the repo public):** the Cargo manifests say `MIT OR Apache-2.0` as a placeholder, and there are no licence files yet. Many compositors and desktops use GPL-3.0 for the compositor and shell, and a permissive licence for the toolkit so others can build on it.
5. **Name (before any public release):** "Neo" is used by many projects. Check for conflicts before a website, package names or a distribution carry it.
