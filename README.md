# Neo

A cross-platform GUI toolkit in Rust, a suite of desktop apps built with it, and the foundation for the Neo Linux desktop.

Every widget paints through a theme: a flat design with hairline borders and higher contrast, in light and dark schemes (dark uses Monokai Pro colours on #181818), with four accents: royal blue (default), teal, coral and amber. Users can adjust corner radius, text size and motion. **Glass** windows are translucent, and the compositor blurs what is behind them.

## The apps

| App | Package | What it does |
|---|---|---|
| Files | `neo-files` | Browse folders with places, breadcrumbs, back and forward, search, sortable columns and hidden files. Three views: list, compact list, and a grid with picture thumbnails. Double-click or Enter opens with the default app. New folder, rename and Move to Trash (freedesktop.org Trash on Linux), also from a right-click menu. Shift and Command/Ctrl clicks select several entries. Drag entries onto a folder to move them, drop files from other apps to copy them in, and on macOS drag entries out to other apps. Pictures open in Photos and videos in Videos. |
| Terminal | `neo-terminal` | A GPU-drawn terminal using `alacritty_terminal` for emulation and the pty: 256 and true colour, bold, underline, inverse, wide characters, scrollback, mouse selection, copy and paste, bracketed paste and window titles. |
| Settings | `neo-settings` | Colour scheme, accent, corner radius, glass, text size and reduced motion, applied to every open Neo app at once; plus About this computer. |
| System Monitor | `neo-monitor` | Processes with filter, sort, thread counts and End process (with confirmation). Open a process to see its memory, its swap and a live table of its threads with each one's CPU use. Per-core CPU, memory, swap and network history; disk usage. A Sensors page shows temperatures for the CPU, GPU, memory and storage where the hardware reports them, with fan speeds. |
| Calculator | `neo-calculator` | Type expressions with precedence, brackets, powers, `%`, `!`, functions, `π`, `e` and `ans`; the result previews as you type, and a history panel recalls past results. Basic and scientific keypads. |
| Photos | `neo-photos` | Views pictures: scroll or pinch to zoom, drag to pan, rotate, and step through the folder. PNG, JPEG, GIF, WebP, BMP, TIFF and ICO everywhere, playing animated GIFs and WebPs; on macOS also HEIC and anything else the system reads. |
| Videos | `neo-videos` | Plays video with play/pause, seeking and volume. Uses AVFoundation on macOS; `ffmpeg` and `ffplay` on Linux and Windows. |
| Launcher | `neo-launcher` | A search box that opens apps. ⌘' (Ctrl+' on Windows) brings it up from anywhere. The search is fuzzy and always shows the closest matches, and apps opened often rank higher. On Linux, bind `neo-launcher` to a key in the window manager. |
| Recorder | `neo-recorder` | Records the full screen, one window, or an area you frame by dragging and resizing a see-through outline. While recording, a small bar shows the time with Pause and Stop, and is left out of the recording where the system allows. Options for the microphone and for saving a GIF. ⌘⇧S (Ctrl+Shift+S) takes a screenshot of the same three kinds. It keeps an icon in the menu bar or system tray, and the installed app starts at login (a setting, on by default) so the shortcut is always ready. ⌘⇧R (Ctrl+Shift+R elsewhere) brings up the area frame from anywhere and stops a recording. Uses the system's `screencapture` on macOS, `wf-recorder` or `ffmpeg` on Linux and `ffmpeg` on Windows. |
| Neo Code | `neo-code` | A code editor with a file tree, tabs, syntax highlighting and optional Vim keys. |

```sh
cargo run -p neo-files [folder]
cargo run -p neo-terminal [folder]
cargo run -p neo-settings
cargo run -p neo-monitor
cargo run -p neo-calculator
cargo run -p neo-photos [picture or folder]
cargo run -p neo-videos [video]
cargo run -p neo-recorder [--mode area --record --for 10 --gif]
cargo run -p neo-code [folder]              # Cmd/Ctrl+S saves
cargo run -p neo-files -- --snapshot target/snapshots   # every app renders PNGs this way
cargo run -p neo --example gallery          # the widget gallery
cargo test --workspace
```

The apps share their appearance through `appearance.conf` in `~/.config/neo` (`%APPDATA%\Neo` on Windows; `$XDG_CONFIG_HOME` and `$NEO_CONFIG_DIR` override it). Settings writes it; every app checks it about once a second and restyles itself.

### Install

```sh
cargo xtask install     # build the apps and add them to this computer's app launcher
cargo xtask uninstall   # remove them
```

| | Where they go | Where they appear |
|---|---|---|
| macOS | `~/Applications/Neo Files.app` and so on | Launchpad, Spotlight, the Dock and Finder |
| Linux | `~/.local/bin`, with `.desktop` entries and icons under `~/.local/share` (set `PREFIX` to change) | The app menu of GNOME, KDE Plasma or any freedesktop.org desktop |
| Windows | `%LOCALAPPDATA%\Programs\Neo` | The Start menu's Neo folder |

On macOS and Windows the apps are named Neo Files, Neo Terminal and so on, so they don't sit next to the system's own Files and Terminal with the same name. `cargo xtask icons` redraws the app icons in `dist/icons` with `armature-render`.

## Writing an app

Neo uses the Elm architecture. State lives in your type, `update` applies messages, and `view` describes the interface.

```rust
use neo::prelude::*;

#[derive(Default)]
struct Counter { n: i32 }

#[derive(Clone)]
enum Msg { Inc, Dec }

impl App for Counter {
    type Message = Msg;
    fn update(&mut self, m: Msg) {
        match m { Msg::Inc => self.n += 1, Msg::Dec => self.n -= 1 }
    }
    fn view(&self) -> Element<Msg> {
        row().spacing(12.0)
            .push(button("−").on_press(Msg::Dec))
            .push(text(self.n.to_string()).role(TextRole::Heading))
            .push(button("+").on_press(Msg::Inc))
            .into()
    }
}

fn main() -> Result<(), neo::Error> { neo::run(Counter::default()) }
```

## Crates

Neo is built on [Armature](https://github.com/mcdearman/armature), a GUI framework with no look of its own, which supplies the windows, input, layout, renderer and test harness. Clone it next to this repository (`../armature`); the workspace finds it there.

| Crate | Role |
|---|---|
| `neo-theme` | Neo's design tokens: palettes, surface materials, type scale, bundled fonts and 1,539 Lucide icons. |
| `neo` | The Neo toolkit: themed controls (buttons, inputs, toggles, sliders, menus, the code editor), the Neo title bar, and the `App` trait the apps implement, built on `armature`. |
| `neo-desktop` | What the apps share: the appearance file and live reload, file helpers (standard folders, sizes, dates, Trash, open with the default app) and common layout pieces such as the sidebar and settings rows. |
| `apps/*` | The desktop apps above, one binary each. |

### Widgets

Text, Icon, Picture, Container, Row, Column, Stack, Space, Divider, Scrollable, MouseArea, PopupMenu, Button (raised, accent, ghost, round), Toggle, Checkbox, Slider, Segmented, TextInput, TextEditor, ProgressBar, Gauge and Sparkline.

TextEditor edits an app-owned `Document`. It supports undo and redo, auto-indent, word motions, the clipboard, and syntax highlighting for Rust, TOML and Markdown.

Call `Document::set_vim(true)` for Vim-style editing:

- **Modes:** Normal, Insert, Visual, Visual Line and Visual Block (`Ctrl-v`, with `I`, `A`, `c`, `r` and block paste).
- **Editing:** counts; `d c y > < gu gU g~` with motions and text objects; `x D C s S r J gJ ~ p P`; `u` and `Ctrl-r`; `.` repeat, including Visual changes and a new count; counted inserts such as `3ix<Esc>`.
- **Registers:** `"a`–`"z` (uppercase appends), `"0`–`"9`, `"-`, `"_`, `".`, `":`, `"/`, and `"+`/`"*` for the system clipboard. `:set clipboard=unnamedplus` makes `y` and `p` use the clipboard. In Insert mode, `Ctrl-r {reg}` pastes a register.
- **Macros and marks:** `q{reg}`…`q`, `@{reg}`, `@@`, `@:`; `m{a-z}`, `'x`, `` `x ``, `''`, `'<`, `'>`, `'.`, `gv`, `gi`.
- **Search and Ex:** `/ ? n N * #` with Rust regex syntax plus Vim's `\<`, `\>` and `\c`; `:s/pat/rep/gi` with `&`, `\1` and `\n`; ranges (`%`, `N,M`, `.`, `$`, `'<,'>`, `+N`); `:d :y :> :< :j :{line}`; `:set ic scs`; `:reg`, `:marks`; `:w :q :q! :wq :x`.
- **Scrolling:** `Ctrl-d u f b e y`, `zz zt zb` and `H M L`, using the editor's real height.

The application carries out `:w` and `:q` through `Document::take_vim_requests`.

### How it works

- **Surfaces, not colours.** Widgets ask the theme to paint a role such as card, raised, pressed, well, inset or accent. How each role looks is decided in one place, in `Theme::paint`.
- **Windows that follow app state.** `App::window_state` says whether the window is shown, on top, or bare and see-through; `on_close` lets an app hide instead of quitting; `on_window_frame` tells it where the window is. Recorder uses all three.
- **Messages from anywhere.** `App::start` hands the app a `Proxy` that sends messages from other threads, such as the terminal's pty reader, and wakes the event loop. Widgets can report layout results, such as the terminal's size in cells, with `Cx::defer`.
- **Widget state survives rebuilds.** Hover, focus, caret and animation state is keyed by tree position, or by an explicit `.key()`.
- **Browser-style blending.** The canvas is sRGB-encoded and blends in sRGB space, so it matches the HTML mock-ups.
- **Accessibility.** Keyboard focus with Tab and Shift+Tab, visible focus rings, contrast tested against WCAG AA, larger text and reduced motion.

## Platform support

| | Rendering | Glass blur | Title bar |
|---|---|---|---|
| Linux, Wayland | Vulkan / GL | KDE blur protocol over the whole window for now; translucent elsewhere | Neo, with edge resizing |
| Linux, X11 | Vulkan / GL | Translucent, no blur | Neo, with edge resizing |
| macOS | Metal | Clear Liquid Glass on macOS 26+, HUD blur before that; both clipped to Neo's corner radius | Neo, with native shadow and resizing |
| Windows 10/11 | DX12 / Vulkan | Acrylic (Windows 11 rounds it with its own 8px corner) | Neo, with edge resizing |

Blur strength is adjustable on macOS and for in-window glass everywhere. Windows acrylic and KDE's blur have no strength setting. On macOS, Neo sets the radius on the system glass's backdrop filter. That key is undocumented, so if a macOS update renames it, the slider stops affecting the window blur and the system default applies.

Tested so far: macOS, both running and with the test suite. Linux and Windows type-check (`cargo check --target …`) but have not been run yet.

## Roadmap

See [docs/ROADMAP.md](docs/ROADMAP.md): from apps that run on any distro, to a Neo session on an existing distro, to a Neo distribution.
