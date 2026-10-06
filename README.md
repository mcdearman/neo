# Neo

A cross-platform GUI toolkit in Rust, a suite of desktop apps built with it, and the foundation for the Neo Linux desktop.

Every widget paints through a theme: a flat design with hairline borders and higher contrast, in light and dark schemes (dark uses Monokai Pro colours on #181818), with four accents: royal blue (default), teal, coral and amber. Users can adjust corner radius, text size and motion. **Glass** windows are translucent, and the compositor blurs what is behind them.

## The apps

| App | Package | What it does |
|---|---|---|
| Files | `neo-files` | Browse folders with places, breadcrumbs, back and forward, search, sortable columns and hidden files. Three views: list, compact list, and a grid with picture thumbnails. Double-click or Enter opens with the default app. New folder, rename and Move to Trash (freedesktop.org Trash on Linux), also from a right-click menu. Shift and Command/Ctrl clicks select several entries. Drag entries onto a folder to move them, drop files from other apps to copy them in, and on macOS drag entries out to other apps. Pictures open in Photos and videos in Videos. |
| NeoTerm | `neo-terminal` | A GPU-drawn terminal using `alacritty_terminal` for emulation and the pty: 256 and true colour, bold, underline, inverse, wide characters, scrollback, mouse selection, copy and paste, bracketed paste and window titles. |
| Settings | `neo-settings` | Colour scheme, accent, corner radius, glass, text size and reduced motion, applied to every open Neo app at once; plus About this computer. |
| System Monitor | `neo-monitor` | Processes with filter, sort, thread counts and End process (with confirmation). Open a process to see its memory, its swap and a live table of its threads with each one's CPU use. Per-core CPU, memory, swap and network history; disk usage. A Sensors page shows temperatures for the CPU, GPU, memory and storage where the hardware reports them, with fan speeds. On macOS a process's memory is its physical footprint, the figure Activity Monitor shows, which counts compressed and swapped-out memory as well as what is in RAM. |
| NeoCal | `neo-calculator` | Type expressions with precedence, brackets, powers, `%`, `!`, functions, `π`, `e` and `ans`; the result previews as you type, and a history panel recalls past results. Basic and scientific keypads. |
| Photos | `neo-photos` | Views pictures: scroll or pinch to zoom, drag to pan, rotate, and step through the folder. PNG, JPEG, GIF, WebP, BMP, TIFF and ICO everywhere, playing animated GIFs and WebPs; on macOS also HEIC and anything else the system reads. |
| Videos | `neo-videos` | Plays video with play/pause, seeking and volume. Uses AVFoundation on macOS; `ffmpeg` and `ffplay` on Linux and Windows. |
| Launcher | `neo-launcher` | A search box that opens apps. ⌘' (Ctrl+' on Windows) brings it up from anywhere. The search is fuzzy and always shows the closest matches, and apps opened often rank higher. On Linux, bind `neo-launcher` to a key in the window manager. |
| NeoCap | `neo-recorder` | Records the full screen, one window, or an area you frame by dragging and resizing a see-through outline. While recording, a small bar shows the time with Pause and Stop, and is left out of the recording where the system allows. Options for the microphone and for saving a GIF. ⌘⇧S (Ctrl+Shift+S) takes a screenshot of the same three kinds. It keeps an icon in the menu bar or system tray, and the installed app starts at login (a setting, on by default) so the shortcut is always ready. ⌘⇧R (Ctrl+Shift+R elsewhere) brings up the area frame from anywhere and stops a recording. Uses the system's `screencapture` on macOS, `wf-recorder` or `ffmpeg` on Linux and `ffmpeg` on Windows. |
| NeoCode | `neo-code` | A code editor with a file tree, tabs, syntax highlighting and optional Vim or Helix keys. Opens a folder from the Explorer, with Cmd/Ctrl+O, by dropping it on the window, or as a command-line argument. |

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
| macOS | `~/Applications/Files.app` and so on | Launchpad, Spotlight, the Dock and Finder |
| Linux | `~/.local/bin`, with `.desktop` entries and icons under `~/.local/share` (set `PREFIX` to change) | The app menu of GNOME, KDE Plasma or any freedesktop.org desktop |
| Windows | `%LOCALAPPDATA%\Programs\Neo` | The Start menu's Neo folder |

The apps have the same names everywhere. The ones named for what they do (Files, Photos, Settings) can sit beside a system app of the same name; their IDs (`org.neo.Photos` and so on) keep them apart, and Neo apps open each other by path, never by name. Installing clears away copies under the earlier "Neo …" names. `cargo xtask icons` redraws the app icons in `dist/icons` with `armature-render`.

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

An app's menu bar comes from `App::menus`: a list of `Menu`s holding `MenuEntry`s, each with an optional `Shortcut`. macOS shows them in its own menu bar at the top of the screen; on Windows and Linux Neo draws them in the title bar. Entries without a message are greyed out, and a shortcut works whether or not its menu is open. `App::app_menu` holds entries about the app as a whole, such as Settings: macOS puts them under the app's name, and elsewhere they follow the first menu.

Every Neo app has **Settings…** there (Cmd/Ctrl+,), opening a panel with that app's own settings. One of them is in every app: **Glass window**, which makes that one app's window solid while the rest of the desktop stays glass. `neo_desktop::Desktop` supplies the entry, the panel and the setting; an app adds its own rows.

TextEditor edits an app-owned `Document`. It supports undo and redo, auto-indent, word motions, the clipboard, and syntax highlighting for Rust, TOML and Markdown.

A document can be driven by standard, Vim or Helix keys: `Document::set_keymap(Keymap::Vim)`, and `Document::mode_status` gives a status bar the mode, pending keys and messages for either.

**Vim** (`Keymap::Vim`):

- **Modes:** Normal, Insert, Visual, Visual Line and Visual Block (`Ctrl-v`, with `I`, `A`, `c`, `r` and block paste).
- **Editing:** counts; `d c y > < gu gU g~` with motions and text objects; `x D C s S r J gJ ~ p P`; `u` and `Ctrl-r`; `.` repeat, including Visual changes and a new count; counted inserts such as `3ix<Esc>`.
- **Registers:** `"a`–`"z` (uppercase appends), `"0`–`"9`, `"-`, `"_`, `".`, `":`, `"/`, and `"+`/`"*` for the system clipboard. `:set clipboard=unnamedplus` makes `y` and `p` use the clipboard. In Insert mode, `Ctrl-r {reg}` pastes a register.
- **Macros and marks:** `q{reg}`…`q`, `@{reg}`, `@@`, `@:`; `m{a-z}`, `'x`, `` `x ``, `''`, `'<`, `'>`, `'.`, `gv`, `gi`.
- **Search and Ex:** `/ ? n N * #` with Rust regex syntax plus Vim's `\<`, `\>` and `\c`; `:s/pat/rep/gi` with `&`, `\1` and `\n`; ranges (`%`, `N,M`, `.`, `$`, `'<,'>`, `+N`); `:d :y :> :< :j :{line}`; `:set ic scs`; `:reg`, `:marks`; `:w :q :q! :wq :x`.
- **Scrolling:** `Ctrl-d u f b e y`, `zz zt zb` and `H M L`, using the editor's real height.

**Helix** (`Keymap::Helix`), where you select first and then act:

- **Modes:** Normal, Insert and Select (`v`). Outside Insert there is always a selection at least one character wide.
- **Moving and selecting:** counts; `h j k l`; `w b e W B E`, which select what they pass; `f t F T`; `gg ge gh gl gs gt gc gb` and `G`; `x` and `X` for lines; `%` for everything; `;` to collapse and `Alt-;` to flip; `mm`, and the text objects `mi` and `ma` with `w W p ( [ { < " ' `` ` ``.
- **Changing:** `i a I A o O`; `d` and `c` (`Alt-d` and `Alt-c` skip the yank); `y p P R`; `r`; `~`, `` ` `` and ``Alt-` `` for case; `> <`; `J`; `u` and `U`; surround with `ms`, `md` and `mr`.
- **Registers and clipboard:** `"a` and other named registers; `Space y`, `Space p`, `Space P` and `Space R` for the system clipboard.
- **Search and commands:** `/ ? n N *` with Rust regex syntax; `:w :q :q! :wq :x` and `:{line}`.
- **Scrolling:** `Ctrl-d u f b`, `zz zt zb zj zk`.
- **Several selections:** every motion and change applies to each one, and they merge when they come to overlap. `C` and `Alt-C` copy a selection to the next or previous line; `s` selects matches of a pattern inside the selections and `S` splits on them; `Alt-s` splits into lines; `_` trims blank space; `,` keeps only the main selection and `Alt-,` drops it; `(` and `)` choose which is the main one. Yanking takes one piece per selection and pasting gives each its own. In Select mode `n` adds the next match.
- **Not included:** macros; the jump list; `.` repeat; aligning selections (`&`).

The application carries out `:w` and `:q` from either keymap through `Document::take_vim_requests`.

### Language servers in NeoCode

NeoCode looks for a language server for each file it opens and starts it for the folder that is open. Servers work on files on disk, so the built-in sample project has none, and the status bar says so. NeoCode reopens the folder from last time, and `neo-code path/to/file` opens a file with its project around it. There is nothing to configure: it searches your `PATH`, the `PATH` your login shell sets up (an app started from a dock or launcher gets a bare one), and the usual install folders such as `~/.cargo/bin`, `~/.local/bin`, Homebrew and Mason. If a copy will not start, such as a rustup proxy without the component behind it, it tries the next. **Settings…** lists what was found.

| Files | Server looked for |
|---|---|
| Rust | `rust-analyzer` |
| C, C++, Objective-C | `clangd` |
| Go | `gopls` |
| Python | `basedpyright-langserver`, `pyright-langserver`, `pylsp`, `ruff server` |
| TypeScript, JavaScript | `typescript-language-server`, `vtsls`, `deno lsp` |
| TOML, Markdown, JSON, YAML, HTML, CSS | `taplo`, `marksman`, and the `vscode-*-language-server` and `yaml-language-server` programs |
| Lua, Zig, shell scripts | `lua-language-server`, `zls`, `bash-language-server` |

With a server running:

- **Problems** are underlined as you type, counted in the status bar, and described there for the line the caret is on.
- **Colours** come from the server (semantic tokens) on top of the built-in highlighter, so languages Neo does not colour itself are coloured too.
- **Completions** appear as you type a word or one of the server's trigger characters: Up and Down choose, Enter or Tab accepts, Escape dismisses. **Go ▸ Complete** (Cmd/Ctrl+.) asks for them.
- **Go ▸ Show Hover** (Cmd/Ctrl+I) shows what is under the caret, and **Go ▸ Go to Definition** (Cmd/Ctrl+G) jumps to where it is defined, when that is in the open folder.
- **Edit ▸ Format Document** (Shift+Cmd/Ctrl+L) formats the file as one undo step.

The text editor widget does the drawing through `TextEditor::marks`, `tokens` and `popup`; the protocol client and the search live in `apps/code/src/lsp`.

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
